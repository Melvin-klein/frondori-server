//! Module match runner : boucle d'UN match, entre autant de participants
//! que l'environnement a d'agents (deux équipes au football, deux chefs en
//! cuisine...). Spawné par le gateway en tâche tokio indépendante, une par
//! match.
//!
//! Ne contient aucune règle de jeu : l'environnement tourne dans un worker
//! séparé (cf. `crate::environments`), ce module ne fait que relayer
//! observations et actions entre les participants et ce worker.
//!
//! **Pas-à-pas** : à chaque pas, le serveur attend l'action de CHAQUE agent
//! avant d'avancer (comme `env.step(actions)` en local), si bien que la
//! latence réseau d'un participant ne lui coûte rien — elle ne fait que
//! rallonger le match. Ce qui est limité, c'est le temps de calcul que
//! l'agent déclare avec chaque action (budget fixé par l'environnement,
//! cf. `timing`). La cadence de l'environnement reste un plancher : un match
//! ne va jamais plus vite que son rythme nominal (diffusion en direct).

pub mod timing;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::future::join_all;
use protocol::{ActionStatus, ServerMessage, Value};
use tokio::sync::broadcast;
use tokio::time::Instant;

use self::timing::SeatTiming;
use crate::environments::{EnvInfo, EnvWorker, WorkerCommand};
use crate::gateway::state::PendingPlayer;
use crate::matches::{MatchStatus, MatchStore, Participant, ParticipantResult};

/// Identifiant public d'un match, généré par le gateway au moment de
/// l'appariement. `Uuid` : unique sans coordination centrale, adapté à un
/// identifiant exposé publiquement (URL `/spectate/:match_id`, colonne `id`
/// de la table `matches`).
pub type MatchId = uuid::Uuid;

/// Informations "administratives" d'un match (identité, environnement,
/// diffusion, persistance), réunies pour ne pas passer dix paramètres
/// positionnels à `run_match`.
pub struct MatchContext {
    pub match_id: MatchId,
    /// Identifiant de l'environnement joué (ex: `"kitchen-v0"`).
    pub environment: String,
    pub env: EnvInfo,
    pub worker_command: WorkerCommand,
    pub match_store: Arc<dyn MatchStore>,
    /// Scènes JSON diffusées aux spectateurs. `Arc<str>` plutôt que
    /// `String` : un `broadcast` clone la valeur pour CHAQUE spectateur, et
    /// cloner un `Arc` ne copie pas la chaîne, juste un compteur.
    pub spectator_tx: broadcast::Sender<Arc<str>>,
}

/// Paramètres de la BOUCLE réseau, communs à tous les matchs.
#[derive(Debug, Clone)]
pub struct MatchRunnerConfig {
    /// Impose une cadence (pas par seconde) à tous les matchs, à la place de
    /// celle déclarée par l'environnement. `None` en production : un match
    /// se joue au rythme de son environnement. Sert aux tests et aux outils
    /// de dev, pour jouer un match complet en quelques secondes.
    pub tick_rate_override: Option<f64>,
    /// Délai RÉSEAU au-delà duquel une action n'est plus attendue : il ne
    /// sert qu'à ne pas bloquer le match sur un client planté. Large, pour
    /// qu'aucun réseau lent ne coûte d'action (le temps de calcul, lui, est
    /// limité à part, par le budget de l'environnement). Au-delà, l'agent
    /// reçoit l'action neutre pour ce pas (jamais sa dernière action
    /// répétée : l'action neutre signale sans ambiguïté "pas de réponse").
    pub response_timeout: Duration,
}

impl Default for MatchRunnerConfig {
    fn default() -> Self {
        Self {
            tick_rate_override: None,
            response_timeout: Duration::from_secs(2),
        }
    }
}

/// Toutes les combien de pas mesurer l'aller-retour réseau (cf. `probe_rtt`).
const RTT_PROBE_EVERY: u32 = 100;
/// Délai d'attente d'un `Pong` pendant une mesure d'aller-retour.
const RTT_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Un participant, du point de vue de la boucle de match.
struct Seat {
    agent: String,
    player_id: String,
    socket: WebSocket,
    /// Encore en jeu : ni terminé ni tronqué. On attend ses actions et on lui
    /// envoie des observations.
    playing: bool,
    /// Son socket fonctionne encore.
    connected: bool,
    /// Déconnecté PENDANT qu'il jouait : le match s'arrête (pas de
    /// reconnexion). Une déconnexion après sa dernière observation n'est pas
    /// un forfait, le match était déjà fini pour lui.
    forfeited: bool,
    /// Sort de l'action attendue pour le tick en cours, renvoyé avec la
    /// prochaine observation (cf. `protocol::ActionStatus`).
    last_action: ActionStatus,
    total_return: f64,
    last_info: Value,
    timing: SeatTiming,
}

/// Boucle principale d'un match. `players` : les participants appariés,
/// dans l'ordre d'arrivée, qui devient l'ordre des agents de
/// l'environnement (le premier contrôle `team_0`, le second `team_1`...).
///
/// `players` est pris PAR VALEUR : cette fonction devient l'unique
/// propriétaire des sockets pour toute la durée du match.
pub async fn run_match(players: Vec<PendingPlayer>, config: MatchRunnerConfig, seed: u64, context: MatchContext) {
    let match_id = context.match_id;

    let mut worker = match EnvWorker::spawn(&context.worker_command) {
        Ok(worker) => worker,
        Err(err) => return abandon_before_start(match_id, players, &err.to_string()).await,
    };
    let start = match worker.start(&context.environment, seed).await {
        Ok(start) => start,
        Err(err) => return abandon_before_start(match_id, players, &err.to_string()).await,
    };
    if start.agents.len() != players.len() {
        let reason = format!("{} agents annoncés pour {} participants", start.agents.len(), players.len());
        return abandon_before_start(match_id, players, &reason).await;
    }

    let mut seats: Vec<Seat> = start
        .agents
        .iter()
        .zip(players)
        .map(|(agent, player)| Seat {
            agent: agent.clone(),
            player_id: player.id,
            socket: player.socket,
            playing: true,
            connected: true,
            forfeited: false,
            last_action: ActionStatus::NotExpected,
            total_return: 0.0,
            last_info: start.infos.get(agent).cloned().unwrap_or(Value::Nil),
            timing: SeatTiming::default(),
        })
        .collect();

    let participants: Vec<Participant> = seats
        .iter()
        .enumerate()
        .map(|(seat, s)| Participant {
            seat,
            agent: s.agent.clone(),
            player_id: s.player_id.clone(),
        })
        .collect();
    context
        .match_store
        .record_start(match_id, &context.environment, &participants)
        .await;

    // `replay` accumule toutes les scènes du match (une par pas), pour un
    // replay complet une fois le match terminé — uniquement ces données,
    // jamais de vidéo.
    let mut replay = Vec::new();
    publish(&context, &mut replay, start.scene);

    let match_start: Vec<Option<ServerMessage>> = seats
        .iter()
        .map(|seat| {
            Some(ServerMessage::MatchStart(protocol::MatchStart {
                match_id: match_id.to_string(),
                environment: context.environment.clone(),
                agent: seat.agent.clone(),
                agents: start.agents.clone(),
                observation_space: space_of(&context.env.observation_spaces, &seat.agent),
                action_space: space_of(&context.env.action_spaces, &seat.agent),
                compute_budget_ms: context.env.compute_budget_ms,
            }))
        })
        .collect();
    send_all(&mut seats, match_start).await;

    let initial: Vec<Option<ServerMessage>> = seats
        .iter()
        .map(|seat| {
            let observation = start.observations.get(&seat.agent).cloned().unwrap_or(Value::Nil);
            Some(observation_message(0, observation, seat.last_action, 0.0, false, false, seat.last_info.clone()))
        })
        .collect();
    // Instant d'envoi des dernières observations : point de départ des
    // temps de réponse mesurés par le serveur.
    let mut observations_sent_at = Instant::now();
    send_all(&mut seats, initial).await;

    // `tokio::time::interval` : un "top" au rythme nominal, qui n'est qu'un
    // PLANCHER (un pas ne dure jamais moins), puisqu'on attend de toute façon
    // les actions de tous les agents. `MissedTickBehavior::Delay` (plutôt que
    // le `Burst` par défaut) : après un pas lent (réseau lointain), on ne
    // rattrape PAS en enchaînant plusieurs pas d'affilée.
    let tick_rate = config.tick_rate_override.unwrap_or(context.env.tick_rate);
    let tick_period = Duration::from_secs_f64(1.0 / tick_rate);
    let budget_ms = context.env.compute_budget_ms;
    let mut ticker = tokio::time::interval(tick_period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // le premier "top" est immédiat

    let mut tick: u32 = 0;
    let status = loop {
        if !seats.iter().any(|seat| seat.playing) {
            break MatchStatus::Finished;
        }
        if seats.iter().any(|seat| seat.forfeited) {
            break MatchStatus::Finished;
        }

        tick += 1;

        // Réception en parallèle : chaque agent a le même délai pour
        // répondre, quel que soit le nombre de participants. `iter_mut`
        // donne un emprunt mutable DISTINCT par participant, ce qui permet
        // d'attendre tous leurs sockets en même temps.
        // On attend la réponse à la DERNIÈRE observation envoyée, celle du
        // tick précédent (l'observation initiale porte le tick 0), de la part
        // de chaque agent — et, en même temps, le top du rythme nominal :
        // le pas avance quand les deux sont là (`tokio::join!`).
        let expected_tick = tick - 1;
        let deadline = observations_sent_at + config.response_timeout;
        let (received, _) = tokio::join!(
            join_all(seats.iter_mut().map(|seat| async move {
                if !seat.playing {
                    return Received::NotExpected;
                }
                recv_action(&mut seat.socket, expected_tick, deadline).await
            })),
            ticker.tick(),
        );

        let mut actions: HashMap<String, Option<Value>> = HashMap::new();
        for (seat, received) in seats.iter_mut().zip(received) {
            match received {
                Received::Action { action, compute_ms, received_at } => {
                    let response_ms = (received_at - observations_sent_at).as_secs_f64() * 1000.0;
                    // Une déclaration absurde (négative, NaN...) ne vaut pas
                    // mieux qu'une action invalide.
                    let (status, applied) = if !compute_ms.is_finite() || compute_ms < 0.0 {
                        seat.timing.record_action(None, response_ms, false);
                        (ActionStatus::Rejected, None)
                    } else if compute_ms > budget_ms {
                        seat.timing.record_action(Some(compute_ms), response_ms, true);
                        (ActionStatus::TooSlow, None)
                    } else {
                        seat.timing.record_action(Some(compute_ms), response_ms, false);
                        (ActionStatus::Applied, Some(action))
                    };
                    seat.last_action = status;
                    actions.insert(seat.agent.clone(), applied);
                }
                Received::Nothing => {
                    seat.timing.record_missing();
                    seat.last_action = ActionStatus::Missing;
                    actions.insert(seat.agent.clone(), None);
                }
                Received::Disconnected => {
                    seat.connected = false;
                    seat.forfeited = true;
                }
                Received::NotExpected => {}
            }
        }
        if seats.iter().any(|seat| seat.forfeited) {
            continue; // le haut de la boucle arrête le match
        }
        // Tous viennent de répondre et attendent la prochaine observation :
        // le bon moment pour mesurer leur aller-retour réseau sans que leur
        // temps de calcul s'y mêle.
        if tick == 1 || tick.is_multiple_of(RTT_PROBE_EVERY) {
            probe_rtt(&mut seats).await;
        }

        let step = match worker.step(&actions).await {
            Ok(step) => step,
            Err(err) => {
                tracing::error!(%match_id, tick, %err, "l'environnement a échoué, match abandonné");
                break MatchStatus::Aborted;
            }
        };
        for seat in seats.iter_mut().filter(|seat| step.rejected.contains(&seat.agent)) {
            seat.last_action = ActionStatus::Rejected;
        }
        publish(&context, &mut replay, step.scene);

        let messages: Vec<Option<ServerMessage>> = seats
            .iter_mut()
            .map(|seat| {
                // Seuls les agents encore en jeu au début de ce pas ont une
                // observation (convention PettingZoo).
                let observation = step.observations.get(&seat.agent)?.clone();
                let reward = step.rewards.get(&seat.agent).copied().unwrap_or(0.0);
                let terminated = step.terminations.get(&seat.agent).copied().unwrap_or(false);
                let truncated = step.truncations.get(&seat.agent).copied().unwrap_or(false);
                seat.total_return += reward;
                if let Some(info) = step.infos.get(&seat.agent) {
                    seat.last_info = info.clone();
                }
                if terminated || truncated {
                    seat.playing = false;
                }
                Some(observation_message(
                    tick,
                    observation,
                    seat.last_action,
                    reward,
                    terminated,
                    truncated,
                    seat.last_info.clone(),
                ))
            })
            .collect();
        observations_sent_at = Instant::now();
        send_all(&mut seats, messages).await;
    };

    // Le match est fini pour tout le monde : un envoi raté du `MatchEnd`
    // ci-dessous ne doit plus pouvoir compter comme un forfait.
    for seat in &mut seats {
        seat.playing = false;
    }
    let forfeited: Vec<String> = seats.iter().filter(|s| s.forfeited).map(|s| s.agent.clone()).collect();
    let returns: HashMap<String, f64> = seats.iter().map(|s| (s.agent.clone(), s.total_return)).collect();
    let results: Vec<ParticipantResult> = seats
        .iter()
        .enumerate()
        .map(|(seat, s)| ParticipantResult {
            seat,
            final_return: s.total_return,
            forfeited: s.forfeited,
            final_info: serde_json::to_value(&s.last_info).unwrap_or(serde_json::Value::Null),
            timing: serde_json::to_value(s.timing.summary(budget_ms)).unwrap_or(serde_json::Value::Null),
            step_timings: s.timing.steps(),
        })
        .collect();
    for s in &seats {
        let summary = s.timing.summary(budget_ms);
        if summary.suspect {
            tracing::warn!(
                %match_id,
                player_id = %s.player_id,
                unexplained_ms = ?summary.median_unexplained_ms,
                rtt_ms = ?summary.rtt_ms,
                mean_compute_ms = ?summary.mean_compute_ms,
                "temps de calcul déclarés incohérents avec les temps de réponse mesurés"
            );
        }
    }
    tracing::info!(%match_id, environment = %context.environment, ticks = tick, status = status.as_str(), ?returns, ?forfeited, "match terminé");

    let end = ServerMessage::MatchEnd(protocol::MatchEnd { returns, forfeited });
    send_all(&mut seats, vec![Some(end); participants.len()]).await;
    // `close` prend le socket PAR VALEUR (fermer une connexion la consomme :
    // on ne peut plus s'en servir après), d'où `for seat in seats` qui
    // consomme la liste, plutôt qu'un `&mut seats`. C'est aussi pour ça que
    // les résultats sont calculés AVANT cette boucle.
    for seat in seats {
        let _ = seat.socket.close().await;
    }

    // Les scènes sont déjà du JSON : le replay est simplement leur liste,
    // assemblée par concaténation, sans jamais les décoder.
    let replay_json = format!("[{}]", replay.join(","));
    context.match_store.record_end(match_id, status, &results, &replay_json).await;
}

/// L'environnement n'a pas pu démarrer : rien n'a été enregistré ni envoyé,
/// on ferme simplement les connexions (le client verra une déconnexion).
async fn abandon_before_start(match_id: MatchId, players: Vec<PendingPlayer>, reason: &str) {
    tracing::error!(%match_id, reason, "impossible de démarrer l'environnement, match annulé");
    for player in players {
        let _ = player.socket.close().await;
    }
}

fn space_of(spaces: &HashMap<String, Value>, agent: &str) -> Value {
    spaces.get(agent).cloned().unwrap_or(Value::Nil)
}

fn publish(context: &MatchContext, replay: &mut Vec<String>, scene: String) {
    // `send` échoue seulement s'il n'y a aucun spectateur abonné : pas une
    // erreur, on l'ignore volontairement (`let _ =`).
    let _ = context.spectator_tx.send(Arc::from(scene.as_str()));
    replay.push(scene);
}

fn observation_message(
    tick: u32,
    observation: Value,
    last_action: ActionStatus,
    reward: f64,
    terminated: bool,
    truncated: bool,
    info: Value,
) -> ServerMessage {
    ServerMessage::Observation(protocol::ObservationMessage {
        tick,
        observation,
        last_action,
        reward,
        terminated,
        truncated,
        info,
    })
}

// ---------------------------------------------------------------------
// Réception des actions
// ---------------------------------------------------------------------

/// Ce qu'on a obtenu d'un participant pour un tick.
enum Received {
    /// Une action pour le bon tick, son temps de calcul déclaré, et l'instant
    /// de sa réception (pour mesurer le temps de réponse).
    Action {
        action: Value,
        compute_ms: f64,
        received_at: Instant,
    },
    /// Aucune action pour ce tick avant le délai réseau : l'action neutre
    /// sera appliquée, sans couper la connexion (un SDK buggé ou ralenti
    /// peut se rattraper au tick suivant).
    Nothing,
    /// Socket fermé ou en erreur : forfait.
    Disconnected,
    /// L'agent a déjà fini son épisode : on n'attend plus rien de lui.
    NotExpected,
}

async fn recv_action(socket: &mut WebSocket, expected_tick: u32, deadline: Instant) -> Received {
    // Une boucle, et non un seul `recv` : le socket peut contenir d'autres
    // messages avant la bonne action, qu'il ne faut surtout pas prendre pour
    // elle. Bug réel corrigé : une action arrivée APRÈS le délai de son tick
    // restait dans le socket et était prise, au tick suivant, pour la
    // réponse courante — l'agent jouait alors avec un pas de retard jusqu'à
    // la fin du match (cf. `tests/late_action_test.rs`). D'où :
    // - une action d'un tick passé est périmée : jetée ;
    // - tout autre message (`Pong` tardif, action d'un tick futur, message
    //   illisible) est ignoré ;
    // et on continue d'attendre la bonne action jusqu'à la MÊME échéance
    // (`timeout_at`, pas un nouveau délai à chaque message).
    loop {
        let message = match tokio::time::timeout_at(deadline, socket.recv()).await {
            Err(_elapsed) => return Received::Nothing,
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return Received::Disconnected,
            Ok(Some(Ok(message))) => message,
        };
        // Texte, ping/pong WebSocket bas niveau : notre protocole n'en envoie
        // jamais côté client, on l'ignore.
        let Message::Binary(bytes) = message else { continue };
        if let Ok(protocol::ClientMessage::Action(action)) = protocol::decode::<protocol::ClientMessage>(&bytes) {
            if action.tick == expected_tick {
                return Received::Action {
                    action: action.action,
                    compute_ms: action.compute_ms,
                    received_at: Instant::now(),
                };
            }
        }
    }
}

/// Mesure l'aller-retour réseau de chaque participant encore en jeu, par un
/// `Ping` dont on attend le `Pong` (en parallèle pour tous). Appelée quand
/// tous viennent de répondre et attendent la prochaine observation : leur
/// client n'a alors rien à calculer et répond tout de suite. Un `Pong` qui
/// n'arrive pas à temps n'est pas une faute, seulement une mesure en moins.
async fn probe_rtt(seats: &mut [Seat]) {
    join_all(seats.iter_mut().filter(|seat| seat.playing && seat.connected).map(|seat| async move {
        let nonce: u64 = rand::random();
        let Ok(ping) = protocol::encode(&ServerMessage::Ping(protocol::Ping { nonce })) else { return };
        let sent_at = Instant::now();
        if seat.socket.send(Message::Binary(ping)).await.is_err() {
            return; // la déconnexion sera constatée au prochain envoi/réception
        }
        let deadline = sent_at + RTT_PROBE_TIMEOUT;
        while let Ok(Some(Ok(message))) = tokio::time::timeout_at(deadline, seat.socket.recv()).await {
            let Message::Binary(bytes) = message else { continue };
            if let Ok(protocol::ClientMessage::Pong(pong)) = protocol::decode::<protocol::ClientMessage>(&bytes) {
                if pong.nonce == nonce {
                    seat.timing.record_rtt(sent_at.elapsed().as_secs_f64() * 1000.0);
                    return;
                }
            }
        }
    }))
    .await;
}

// ---------------------------------------------------------------------
// Envoi
// ---------------------------------------------------------------------

/// Envoie `messages[i]` au participant `i` (rien si `None`), à tous en
/// parallèle. Un envoi qui échoue marque le participant déconnecté ; c'est
/// un forfait seulement s'il était encore en jeu.
async fn send_all(seats: &mut [Seat], messages: Vec<Option<ServerMessage>>) {
    join_all(seats.iter_mut().zip(messages).map(|(seat, message)| async move {
        let Some(message) = message else { return };
        if !seat.connected {
            return;
        }
        let sent = match protocol::encode(&message) {
            Ok(bytes) => seat.socket.send(Message::Binary(bytes)).await.is_ok(),
            Err(err) => {
                tracing::error!(err = %err.0, "échec d'encodage d'un message");
                false
            }
        };
        if !sent {
            seat.connected = false;
            if seat.playing {
                seat.forfeited = true;
            }
        }
    }))
    .await;
}
