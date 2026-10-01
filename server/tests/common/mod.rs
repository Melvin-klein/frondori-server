//! Utilitaires partagés entre les tests d'intégration de `server`.
//!
//! `tests/common/mod.rs` (un sous-dossier avec un `mod.rs`, pas
//! `tests/common.rs`) : c'est la convention Cargo pour qu'un fichier de
//! `tests/` ne devienne PAS lui-même un binaire de test à part entière.
//!
//! Chaque fichier `tests/*.rs` qui fait `mod common;` recompile sa PROPRE
//! copie de ce module et n'en utilise qu'une partie, d'où les
//! `#[allow(dead_code)]` (sinon, faux positifs selon le fichier compilé).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use protocol::Value;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use server::auth::InMemoryAuthProvider;
use server::environments::{describe_environments, WorkerCommand};
use server::match_runner::MatchRunnerConfig;
use server::matches::{MatchStore, NullMatchStore};

/// `(token, player_id)` connus du serveur de test. Un token identifie un
/// agent, qui peut jouer à n'importe quel environnement.
const TOKENS: [(&str, &str); 2] = [("token-a", "agent-a"), ("token-b", "agent-b")];

/// Le worker Python des tests : celui du venv d'`engine-python`, sauf si
/// `FRONDORI_ENV_WORKER` en désigne un autre. Les tests jouent donc de VRAIS
/// environnements, exécutés par de vrais processus — pas un simulacre.
#[allow(dead_code)]
pub fn worker_command() -> WorkerCommand {
    if let Ok(line) = std::env::var("FRONDORI_ENV_WORKER") {
        return WorkerCommand::parse(&line);
    }
    // `env!("CARGO_MANIFEST_DIR")` : dossier du crate `server`, connu à la
    // compilation, pour retrouver `engine-python` quel que soit le dossier
    // depuis lequel on lance `cargo test`.
    let python = concat!(env!("CARGO_MANIFEST_DIR"), "/../engine-python/.venv/bin/python");
    assert!(
        Path::new(python).exists(),
        "worker introuvable ({python}) : construire engine-python d'abord \
         (cd engine-python && python -m venv .venv && source .venv/bin/activate \
         && pip install maturin && maturin develop), ou définir FRONDORI_ENV_WORKER"
    );
    WorkerCommand {
        program: python.to_string(),
        args: vec!["-m".to_string(), "frondori_engine.worker".to_string()],
    }
}

/// Démarre un vrai serveur (vrai port TCP choisi par l'OS) avec les tokens
/// de test et le catalogue réel des environnements. `tick_rate` impose la
/// cadence de tous les matchs, pour qu'un match complet dure une fraction
/// de seconde au lieu de minutes.
#[allow(dead_code)]
pub async fn start_test_server_with(match_store: Arc<dyn MatchStore>, tick_rate: f64) -> SocketAddr {
    let config = MatchRunnerConfig {
        tick_rate_override: Some(tick_rate),
        ..MatchRunnerConfig::default()
    };
    start_test_server_with_config(match_store, config).await
}

/// Comme `start_test_server_with`, avec une configuration de boucle de match
/// complète (ex. un délai réseau court).
#[allow(dead_code)]
pub async fn start_test_server_with_config(match_store: Arc<dyn MatchStore>, config: MatchRunnerConfig) -> SocketAddr {
    let tokens: HashMap<String, String> = TOKENS
        .iter()
        .map(|(token, player_id)| (token.to_string(), player_id.to_string()))
        .collect();

    let worker_command = worker_command();
    let catalog = describe_environments(&worker_command)
        .await
        .expect("le worker n'a pas pu décrire les environnements");
    let state = server::new_app_state(
        Arc::new(InMemoryAuthProvider::new(tokens)),
        match_store,
        catalog,
        worker_command,
        config,
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("impossible de binder un port de test");
    let addr = listener.local_addr().unwrap();
    // Tâche indépendante, abandonnée à la fin du test.
    tokio::spawn(async move {
        axum::serve(listener, server::router(state)).await.unwrap();
    });
    addr
}

/// Serveur de test sans persistance, cadencé à 1000 pas par seconde.
#[allow(dead_code)]
pub async fn start_test_server() -> SocketAddr {
    start_test_server_with(Arc::new(NullMatchStore), 1000.0).await
}

/// Comment se comporte le client de test.
pub struct Behaviour {
    /// Action à envoyer à chaque tick, à partir de l'`action_space` reçu
    /// dans `MatchStart` (par défaut : l'action "zéro" de ce space).
    pub action: fn(&Value) -> Value,
    /// Se déconnecter brutalement après ce tick (pour tester le forfait).
    pub disconnect_after: Option<u32>,
    /// Reçoit l'identifiant du match dès `MatchStart`, pour agir PENDANT le
    /// match (ex. se connecter en spectateur).
    pub match_id_tx: Option<tokio::sync::oneshot::Sender<String>>,
    /// Temps de calcul déclaré avec chaque action.
    pub compute_ms: f64,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            action: zero_action,
            disconnect_after: None,
            match_id_tx: None,
            compute_ms: 1.0,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct MatchReport {
    pub start: protocol::MatchStart,
    pub end: protocol::MatchEnd,
    /// Nombre d'observations reçues (initiale comprise).
    pub observations: u32,
    /// Sort des actions envoyées, tel que rapporté par le serveur.
    pub applied: u32,
    pub rejected: u32,
    pub too_slow: u32,
    pub missing: u32,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum Outcome {
    /// Refusé au handshake (`AuthError`), avec la raison donnée.
    Rejected(String),
    /// Parti volontairement en cours de match (`disconnect_after`).
    Left,
    Finished(MatchReport),
}

/// Joue un match du point de vue d'un client SDK minimal : handshake, puis
/// une action par observation reçue, réponses aux `Ping`, jusqu'au
/// `MatchEnd`. Parle le protocole directement (pas via le SDK Python) : on
/// teste ici le serveur, pas le SDK.
#[allow(dead_code)]
pub async fn play_match(url: String, token: &str, environment: &str, mut behaviour: Behaviour) -> Outcome {
    let (mut ws, _response) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connexion WebSocket échouée");

    let hello = protocol::ClientMessage::Hello(protocol::Hello {
        token: token.to_string(),
        environment: environment.to_string(),
        client_name: "test-client".to_string(),
    });
    ws.send(WsMessage::Binary(protocol::encode(&hello).unwrap()))
        .await
        .expect("envoi du Hello échoué");

    let mut start = None;
    let mut action_space = Value::Nil;
    let (mut observations, mut applied, mut rejected, mut too_slow, mut missing) = (0, 0, 0, 0, 0);
    loop {
        let Some(Ok(WsMessage::Binary(bytes))) = ws.next().await else {
            panic!("connexion fermée avant la fin du match");
        };
        match protocol::decode::<protocol::ServerMessage>(&bytes).unwrap() {
            protocol::ServerMessage::Welcome(_) => {}
            protocol::ServerMessage::AuthError(err) => return Outcome::Rejected(err.reason),
            protocol::ServerMessage::Ping(ping) => {
                let pong = protocol::ClientMessage::Pong(protocol::Pong { nonce: ping.nonce });
                ws.send(WsMessage::Binary(protocol::encode(&pong).unwrap())).await.unwrap();
            }
            protocol::ServerMessage::MatchStart(match_start) => {
                if let Some(tx) = behaviour.match_id_tx.take() {
                    let _ = tx.send(match_start.match_id.clone());
                }
                action_space = match_start.action_space.clone();
                start = Some(match_start);
            }
            protocol::ServerMessage::Observation(observation) => {
                observations += 1;
                match observation.last_action {
                    protocol::ActionStatus::Applied => applied += 1,
                    protocol::ActionStatus::Rejected => rejected += 1,
                    protocol::ActionStatus::TooSlow => too_slow += 1,
                    protocol::ActionStatus::Missing => missing += 1,
                    protocol::ActionStatus::NotExpected => {}
                }
                if behaviour.disconnect_after.is_some_and(|tick| observation.tick >= tick) {
                    drop(ws); // coupure brutale, sans MatchEnd attendu
                    return Outcome::Left;
                }
                if !observation.terminated && !observation.truncated {
                    let action = protocol::ClientMessage::Action(protocol::ActionMessage {
                        tick: observation.tick,
                        action: (behaviour.action)(&action_space),
                        compute_ms: behaviour.compute_ms,
                    });
                    ws.send(WsMessage::Binary(protocol::encode(&action).unwrap())).await.unwrap();
                }
            }
            protocol::ServerMessage::MatchEnd(end) => {
                return Outcome::Finished(MatchReport {
                    start: start.expect("MatchEnd reçu sans MatchStart"),
                    end,
                    observations,
                    applied,
                    rejected,
                    too_slow,
                    missing,
                });
            }
        }
    }
}

/// L'action "zéro" d'un space décrit sur le fil — la même règle que
/// l'action neutre côté worker (`wire.neutral_action`), pour les deux
/// spaces d'action des environnements actuels.
#[allow(dead_code)]
pub fn zero_action(space: &Value) -> Value {
    match field(space, "type").and_then(Value::as_str) {
        Some("discrete") => field(space, "start").cloned().unwrap_or(Value::from(0)),
        Some("box") => {
            let shape: Vec<usize> = field(space, "shape")
                .and_then(Value::as_array)
                .map(|dims| dims.iter().filter_map(Value::as_u64).map(|d| d as usize).collect())
                .unwrap_or_default();
            zeros(&shape)
        }
        other => panic!("space d'action non géré par le client de test : {other:?}"),
    }
}

/// Une action qu'aucun environnement ne peut accepter.
#[allow(dead_code)]
pub fn garbage_action(_space: &Value) -> Value {
    Value::from("pas une action")
}

/// Champ `key` d'une map MessagePack (`None` si absent ou pas une map).
#[allow(dead_code)]
pub fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_map()?.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v)
}

fn zeros(shape: &[usize]) -> Value {
    match shape.split_first() {
        None => Value::from(0.0),
        Some((len, rest)) => Value::Array((0..*len).map(|_| zeros(rest)).collect()),
    }
}
