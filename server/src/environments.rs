//! Les environnements, vus du serveur : un catalogue (quels jeux existent,
//! combien d'agents, à quelle cadence, avec quels spaces) et un worker par
//! match, qui exécute l'environnement dans un processus séparé.
//!
//! Le serveur ne contient AUCUNE règle de jeu : il lance
//! `python -m frondori_engine.worker` (cf. `engine-python/.../worker.py`) et
//! lui parle en MessagePack sur son entrée/sortie standard. Un environnement
//! écrit en Rust (le football) ou en Python (la cuisine) passe par le même
//! chemin, et un environnement qui plante n'emporte que son propre match.

use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use protocol::Value;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// Ce que le serveur sait d'un environnement, tel que décrit par le worker
/// (`cmd: describe`).
#[derive(Debug, Clone, Deserialize)]
pub struct EnvInfo {
    /// Agents d'un match, dans l'ordre de l'environnement. Leur nombre est
    /// le nombre de participants à apparier pour lancer un match.
    pub agents: Vec<String>,
    /// Cadence d'un match, en pas par seconde.
    pub tick_rate: f64,
    /// Comment le site classe les agents : `"elo"` (duel) ou
    /// `"mean_return"` (retour moyen par match). Le serveur ne s'en sert
    /// pas lui-même : il le publie dans la table `environments`.
    pub ranking: String,
    /// Nom et description lisibles, affichés par le site.
    pub title: String,
    pub description: String,
    pub observation_spaces: HashMap<String, Value>,
    pub action_spaces: HashMap<String, Value>,
}

/// Identifiant d'environnement (`"football-v0"`) -> description.
pub type Catalog = HashMap<String, EnvInfo>;

/// Commande qui lance un worker. Configurable (variable d'environnement
/// `FRONDORI_ENV_WORKER`), parce que le bon interpréteur Python dépend de la
/// machine : en dev, celui du venv d'`engine-python`.
#[derive(Debug, Clone)]
pub struct WorkerCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl WorkerCommand {
    pub fn from_env() -> Self {
        let line = std::env::var("FRONDORI_ENV_WORKER")
            .unwrap_or_else(|_| "python3 -m frondori_engine.worker".to_string());
        Self::parse(&line)
    }

    /// Découpe une ligne de commande sur les espaces (pas de guillemets
    /// gérés : suffisant pour `chemin/vers/python -m frondori_engine.worker`).
    pub fn parse(line: &str) -> Self {
        let mut parts = line.split_whitespace().map(str::to_string);
        Self {
            program: parts.next().unwrap_or_default(),
            args: parts.collect(),
        }
    }
}

#[derive(Debug)]
pub struct WorkerError(pub String);

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Réponse du worker à `start` : l'épisode vient d'être réinitialisé.
#[derive(Debug, Deserialize)]
pub struct StartReply {
    pub agents: Vec<String>,
    pub observations: HashMap<String, Value>,
    pub infos: HashMap<String, Value>,
    /// Scène déjà sérialisée en JSON (cf. `frondori_engine/scene.py`) :
    /// relayée telle quelle aux spectateurs et au replay, jamais décodée ici.
    pub scene: String,
}

/// Réponse du worker à `step`. Chaque map ne contient que les agents encore
/// en jeu au début de ce pas (convention PettingZoo).
#[derive(Debug, Deserialize)]
pub struct StepReply {
    pub observations: HashMap<String, Value>,
    pub rewards: HashMap<String, f64>,
    pub terminations: HashMap<String, bool>,
    pub truncations: HashMap<String, bool>,
    pub infos: HashMap<String, Value>,
    pub scene: String,
    /// Agents dont l'action était invalide pour leur `action_space`, et a
    /// donc été remplacée par l'action neutre.
    pub rejected: Vec<String>,
}

/// Requêtes envoyées au worker. `#[serde(tag = "cmd")]` : chaque variante
/// est encodée en map avec un champ `cmd` qui porte son nom, par exemple
/// `{"cmd": "start", "env_id": "...", "seed": 42}` — le format attendu par
/// `worker.py`.
#[derive(Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request<'a> {
    Describe,
    Start { env_id: &'a str, seed: u64 },
    Step { actions: HashMap<&'a str, Option<&'a Value>> },
}

/// Seuls champs communs à toutes les réponses. Décodé AVANT la réponse
/// complète : serde ignore les champs inconnus, on peut donc relire les mêmes
/// octets deux fois, une fois pour savoir si c'est une erreur, une fois pour
/// en extraire le contenu typé.
#[derive(Deserialize)]
struct Envelope {
    ok: bool,
    error: Option<String>,
}

#[derive(Deserialize)]
struct DescribeReply {
    environments: Catalog,
}

/// Délai pour démarrer (lancement de Python, imports, création de
/// l'environnement) : large, ça n'arrive qu'une fois par match.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Délai pour un seul pas : un pas prend quelques centaines de µs (mesuré
/// ~0,2 ms pour le football) ; au-delà de ce délai, l'environnement est
/// considéré bloqué et le match est abandonné.
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// Un worker vivant, dédié à un match. `kill_on_drop` : quand cette valeur
/// est détruite (fin de match, ou tâche du match qui s'arrête pour n'importe
/// quelle raison), le processus est tué avec elle — jamais de worker
/// orphelin. C'est l'ownership Rust qui garantit ce nettoyage, sans aucun
/// `kill()` manuel à ne pas oublier.
pub struct EnvWorker {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl EnvWorker {
    pub fn spawn(command: &WorkerCommand) -> Result<Self, WorkerError> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Les erreurs Python (traceback) apparaissent dans les logs du
            // serveur, sans rien à faire de plus.
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| {
                WorkerError(format!(
                    "impossible de lancer le worker `{} {}` : {err} (cf. FRONDORI_ENV_WORKER)",
                    command.program,
                    command.args.join(" ")
                ))
            })?;

        // `.take()` sort les flux du `Child` pour qu'on en devienne
        // propriétaires ; `expect` : ils existent forcément, on vient de les
        // demander en `Stdio::piped()` juste au-dessus.
        let stdin = child.stdin.take().expect("stdin demandé en piped");
        let stdout = child.stdout.take().expect("stdout demandé en piped");
        Ok(Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    pub async fn describe(&mut self) -> Result<Catalog, WorkerError> {
        let reply: DescribeReply = self.request(&Request::Describe, STARTUP_TIMEOUT).await?;
        Ok(reply.environments)
    }

    pub async fn start(&mut self, env_id: &str, seed: u64) -> Result<StartReply, WorkerError> {
        self.request(&Request::Start { env_id, seed }, STARTUP_TIMEOUT).await
    }

    /// Un pas de simulation. `actions` : pour chaque agent encore en jeu,
    /// son action (`None` s'il n'a pas répondu à temps).
    pub async fn step(&mut self, actions: &HashMap<String, Option<Value>>) -> Result<StepReply, WorkerError> {
        let actions = actions
            .iter()
            .map(|(agent, action)| (agent.as_str(), action.as_ref()))
            .collect();
        self.request(&Request::Step { actions }, STEP_TIMEOUT).await
    }

    /// Envoie une requête et attend sa réponse. Protocole de trame : 4 octets
    /// big-endian pour la taille, puis le message MessagePack (identique des
    /// deux côtés, cf. `_read_frame`/`_write_frame` dans `worker.py`).
    async fn request<T: for<'de> Deserialize<'de>>(
        &mut self,
        request: &Request<'_>,
        timeout: Duration,
    ) -> Result<T, WorkerError> {
        let payload = protocol::encode(request).map_err(|err| WorkerError(err.0))?;

        let exchange = async {
            self.stdin.write_all(&(payload.len() as u32).to_be_bytes()).await?;
            self.stdin.write_all(&payload).await?;
            self.stdin.flush().await?;

            let mut header = [0u8; 4];
            self.stdout.read_exact(&mut header).await?;
            let mut body = vec![0u8; u32::from_be_bytes(header) as usize];
            self.stdout.read_exact(&mut body).await?;
            Ok::<_, std::io::Error>(body)
        };

        let body = tokio::time::timeout(timeout, exchange)
            .await
            .map_err(|_| WorkerError(format!("pas de réponse du worker en {timeout:?}")))?
            .map_err(|err| WorkerError(format!("worker injoignable (arrêté ?) : {err}")))?;

        let envelope: Envelope = protocol::decode(&body).map_err(|err| WorkerError(err.0))?;
        if !envelope.ok {
            return Err(WorkerError(
                envelope.error.unwrap_or_else(|| "erreur sans message".to_string()),
            ));
        }
        protocol::decode(&body).map_err(|err| WorkerError(err.0))
    }
}

/// Démarre un worker le temps de lui demander le catalogue, puis l'arrête.
/// Appelé une fois au démarrage du serveur (cf. `main.rs`).
pub async fn describe_environments(command: &WorkerCommand) -> Result<Catalog, WorkerError> {
    EnvWorker::spawn(command)?.describe().await
}
