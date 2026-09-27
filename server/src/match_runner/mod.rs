//! Module match runner : boucle de simulation d'UN match, entre exactement
//! deux sockets (deux équipes). Spawné par le gateway en tâche tokio
//! indépendante, une par match.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use engine::types::{Action, Actions, Observation, SpectatorFrame};
use engine::{Engine, EngineConfig};
use tokio::sync::broadcast;

use crate::matches::MatchStore;

/// Identifiant public d'un match, généré par le gateway au moment de
/// l'appariement (cf. `gateway::handle_new_connection`). `Uuid` : unique
/// sans coordination centrale (contrairement à un compteur), adapté à un
/// identifiant exposé publiquement (URL `/spectate/:match_id`, colonne `id`
/// de la table `matches`).
pub type MatchId = uuid::Uuid;

/// Regroupe les informations "administratives" d'un match (identité,
/// diffusion spectateur, persistance) — par opposition aux paramètres de
/// SIMULATION (`EngineConfig`) ou de boucle RÉSEAU (`MatchRunnerConfig`).
/// Réunies dans une struct plutôt que passées une par une : `run_match`
/// commençait à accumuler trop de paramètres positionnels distincts pour
/// rester facile à lire/appeler correctement.
pub struct MatchContext {
    pub match_id: MatchId,
    /// Identifiant de l'équipe "home" (= équipe d'indice 0, `socket_a`).
    pub home_player_id: String,
    /// Identifiant de l'équipe "away" (= équipe d'indice 1, `socket_b`).
    pub away_player_id: String,
    pub match_store: Arc<dyn MatchStore>,
    pub spectator_tx: broadcast::Sender<SpectatorFrame>,
}

/// Paramètres de la BOUCLE réseau du match, distincts d'`EngineConfig` (qui
/// ne concerne que la physique/les règles du jeu, aucune notion réseau).
#[derive(Debug, Clone)]
pub struct MatchRunnerConfig {
    /// Fréquence de tick, en Hz (ex: 30 => dt = 1/30s). Doit correspondre à
    /// `EngineConfig::dt` utilisé pour ce même match : c'est ce qui fait
    /// coïncider "un tick de simulation" et "un tick réseau".
    pub tick_rate_hz: u32,
    /// Délai maximum d'attente de l'action d'un agent avant application d'une
    /// action par défaut pour ce tick.
    ///
    /// Décision prise (documentée ici, appliquée dans `recv_action`) : en cas
    /// de timeout, on applique `Action::NOOP` plutôt que de répéter la
    /// dernière action reçue. Raison : une action répétée indéfiniment (ex:
    /// un tir en boucle) serait plus difficile à distinguer d'un agent
    /// volontairement lent/lag-abusif ; NOOP est un signal sans ambiguïté
    /// "cet agent n'a pas répondu ce tick".
    pub action_timeout: Duration,
}

impl Default for MatchRunnerConfig {
    fn default() -> Self {
        Self {
            tick_rate_hz: 30,
            action_timeout: Duration::from_millis(50),
        }
    }
}

/// Boucle principale d'un match.
///
/// `socket_a`/`socket_b` sont pris PAR VALEUR (pas par référence) : cette
/// fonction en devient l'unique propriétaire pour toute la durée du match,
/// aucune autre tâche ne peut y toucher pendant ce temps — cohérent avec la
/// contrainte "pas de reconnexion en cours de match".
pub async fn run_match(
    mut socket_a: WebSocket,
    mut socket_b: WebSocket,
    match_config: MatchRunnerConfig,
    engine_config: EngineConfig,
    seed: u64,
    context: MatchContext,
) {
    let players_per_team = engine_config.players_per_team;
    let mut engine = Engine::new(engine_config, seed);

    // Enregistrement du DÉBUT du match : ne bloque jamais la partie en cas
    // d'échec (cf. doc de `MatchStore::record_start`), donc pas besoin de
    // vérifier un résultat ici.
    context
        .match_store
        .record_start(context.match_id, &context.home_player_id, &context.away_player_id)
        .await;

    if send_match_start(&mut socket_a, &mut socket_b, context.match_id)
        .await
        .is_err()
    {
        tracing::warn!(match_id = %context.match_id, "échec d'envoi de MatchStart, match annulé");
        return;
    }

    // `tokio::time::interval` : déclenche un "top" au rythme voulu
    // (`1 / tick_rate_hz` secondes). `MissedTickBehavior::Delay` (plutôt que
    // le `Burst` par défaut) : si un tick a pris plus de temps que prévu
    // (agent lent, réseau...), on ne cherche PAS à rattraper le retard en
    // enchaînant plusieurs ticks d'affilée — on repart juste sur un rythme
    // normal à partir de maintenant. Plus doux pour les deux agents.
    let tick_period = Duration::from_secs_f64(1.0 / match_config.tick_rate_hz as f64);
    let mut ticker = tokio::time::interval(tick_period);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // le premier "top" est immédiat, on le consomme tout de suite

    // `spectator_frame` donne l'état ABSOLU du match (jamais mirroré), pensé
    // pour un humain qui regarde plutôt que pour un agent. `send` sur un
    // `broadcast::Sender` échoue seulement s'il n'y a aucun abonné (aucun
    // spectateur connecté) : ce n'est pas une erreur, on l'ignore
    // volontairement (`let _ =`).
    //
    // `replay_frames` accumule TOUTES les frames du match, pour permettre un
    // replay complet une fois le match terminé (`record_end`) — aucune
    // vidéo n'est jamais enregistrée, uniquement cette séquence de données.
    // Construit AVANT le premier envoi (pas seulement après un envoi réussi)
    // pour qu'un échec dès ce premier envoi puisse quand même être enregistré
    // via `handle_forfeit`, au lieu de laisser le match bloqué en "live".
    let initial_observations = engine.reset();
    let mut replay_frames = vec![engine.spectator_frame()];
    let _ = context.spectator_tx.send(replay_frames[0].clone());

    let (a_sent, b_sent) = send_both(&mut socket_a, &mut socket_b, 0, &initial_observations).await;
    if !a_sent || !b_sent {
        tracing::warn!(a_failed = !a_sent, b_failed = !b_sent, "échec d'envoi de l'observation initiale");
        handle_forfeit(
            socket_a,
            socket_b,
            !a_sent,
            !b_sent,
            &initial_observations,
            &context,
            &replay_frames,
        )
        .await;
        return;
    }
    let mut last_observations = initial_observations;

    let mut tick: u32 = 0;
    loop {
        ticker.tick().await;
        tick += 1;

        let ((outcome_a, latency_a), (outcome_b, latency_b)) = tokio::join!(
            recv_action(&mut socket_a, match_config.action_timeout, players_per_team),
            recv_action(&mut socket_b, match_config.action_timeout, players_per_team),
        );
        tracing::debug!(tick, ?latency_a, ?latency_b, "actions reçues pour ce tick");

        let a_disconnected = matches!(outcome_a, ActionOutcome::Disconnected);
        let b_disconnected = matches!(outcome_b, ActionOutcome::Disconnected);
        if a_disconnected || b_disconnected {
            handle_forfeit(
                socket_a,
                socket_b,
                a_disconnected,
                b_disconnected,
                &last_observations,
                &context,
                &replay_frames,
            )
            .await;
            return;
        }

        let actions_a = outcome_a.into_actions(players_per_team);
        let actions_b = outcome_b.into_actions(players_per_team);

        let result = engine.step([actions_a, actions_b]);
        last_observations = result.observations.clone();
        let frame = engine.spectator_frame();
        let _ = context.spectator_tx.send(frame.clone());
        replay_frames.push(frame);

        // Même situation qu'à l'envoi initial : si l'un des deux envois
        // échoue (agent déconnecté), on traite ça comme un forfait — plutôt
        // que de logger et abandonner en laissant le match bloqué "live" en
        // base indéfiniment (bug réel rencontré en testant ce module en
        // conditions réelles : un client tué brutalement peut se manifester
        // ici, sur l'ENVOI, plutôt que sur la réception de la prochaine
        // action, selon comment le client meurt exactement).
        let (a_sent, b_sent) = send_both(&mut socket_a, &mut socket_b, tick, &result.observations).await;
        if !a_sent || !b_sent {
            tracing::warn!(tick, a_failed = !a_sent, b_failed = !b_sent, "échec d'envoi de l'observation");
            handle_forfeit(
                socket_a,
                socket_b,
                !a_sent,
                !b_sent,
                &last_observations,
                &context,
                &replay_frames,
            )
            .await;
            return;
        }

        if result.done {
            let outcome_a = outcome_from_score(result.observations[0].score);
            let outcome_b = outcome_from_score(result.observations[1].score);
            let _ = send_match_end(&mut socket_a, outcome_a, result.observations[0].score).await;
            let _ = send_match_end(&mut socket_b, outcome_b, result.observations[1].score).await;
            let _ = socket_a.close().await;
            let _ = socket_b.close().await;
            tracing::info!(tick, score = ?result.observations[0].score, "match terminé");

            let (home_score, away_score) = result.observations[0].score;
            context
                .match_store
                .record_end(context.match_id, home_score, away_score, outcome_a, &replay_frames)
                .await;
            return;
        }
    }
}

// ---------------------------------------------------------------------
// Réception des actions
// ---------------------------------------------------------------------

/// Résultat de la tentative de réception d'une action pour un tick donné.
enum ActionOutcome {
    /// Une action valide, de la bonne taille, a été reçue à temps.
    Received(Actions),
    /// Rien reçu dans le délai imparti, OU un message reçu mais invalide
    /// (mauvais type, mauvaise taille...) : traité comme une non-réponse,
    /// pas comme une erreur fatale.
    TimedOut,
    /// Le socket est fermé ou en erreur : l'agent est considéré déconnecté,
    /// ce qui termine le match par forfait (cf. `handle_forfeit`).
    Disconnected,
}

impl ActionOutcome {
    /// Convertit le résultat en `Actions` utilisables par `Engine::step`,
    /// en remplaçant `TimedOut` par une action neutre pour chaque joueur.
    /// Ne doit jamais être appelée avec `Disconnected` (le match se termine
    /// avant, cf. `run_match`) : `debug_assert!` plutôt qu'un `Result` pour
    /// ne pas complexifier l'API pour un cas déjà exclu par construction.
    fn into_actions(self, players_per_team: usize) -> Actions {
        match self {
            ActionOutcome::Received(actions) => actions,
            ActionOutcome::TimedOut => Actions {
                players: vec![Action::NOOP; players_per_team],
            },
            ActionOutcome::Disconnected => {
                debug_assert!(false, "into_actions appelé sur une équipe déconnectée");
                Actions {
                    players: vec![Action::NOOP; players_per_team],
                }
            }
        }
    }
}

/// Attend un message `protocol::ActionMessage` sur `socket`, borné par
/// `timeout`. Retourne aussi le temps effectivement écoulé, pour le
/// logging de latence (cf. `tracing::debug!` dans `run_match`).
async fn recv_action(
    socket: &mut WebSocket,
    timeout: Duration,
    players_per_team: usize,
) -> (ActionOutcome, Duration) {
    let started = Instant::now();

    let outcome = match tokio::time::timeout(timeout, socket.recv()).await {
        // Le délai s'est écoulé avant toute réponse.
        Err(_elapsed) => ActionOutcome::TimedOut,
        // Le socket est fermé proprement (plus aucun message ne viendra).
        Ok(None) => ActionOutcome::Disconnected,
        // Erreur réseau/protocole WebSocket : on traite comme une déconnexion.
        Ok(Some(Err(_))) => ActionOutcome::Disconnected,
        Ok(Some(Ok(Message::Binary(bytes)))) => {
            match protocol::decode::<protocol::ClientMessage>(&bytes) {
                Ok(protocol::ClientMessage::Action(action_msg))
                    if action_msg.actions.players.len() == players_per_team =>
                {
                    ActionOutcome::Received(action_msg.actions)
                }
                // Message décodable mais de la mauvaise forme (mauvais
                // variant, mauvais nombre de joueurs...) : on ne ferme pas
                // la connexion pour ça, on traite juste ce tick comme non
                // répondu. Un SDK buggé peut ainsi se corriger tout seul au
                // tick suivant sans perdre le match.
                _ => ActionOutcome::TimedOut,
            }
        }
        // Frame WebSocket non-binaire (texte, ping/pong bas niveau...) :
        // notre protocole n'en envoie jamais côté client, on l'ignore.
        Ok(Some(Ok(_))) => ActionOutcome::TimedOut,
    };

    (outcome, started.elapsed())
}

// ---------------------------------------------------------------------
// Envoi des observations / fin de match
// ---------------------------------------------------------------------

/// Envoie l'observation de chaque équipe sur son socket, en parallèle.
/// Retourne `(a_ok, b_ok)` plutôt qu'un simple `Result` : contrairement à un
/// échec de réception (cf. `ActionOutcome::Disconnected`), on a besoin de
/// savoir PRÉCISÉMENT lequel des deux a échoué pour attribuer correctement
/// le forfait (cf. `handle_forfeit`).
async fn send_both(
    socket_a: &mut WebSocket,
    socket_b: &mut WebSocket,
    tick: u32,
    observations: &[Observation; 2],
) -> (bool, bool) {
    let (result_a, result_b) = tokio::join!(
        send_observation(socket_a, tick, &observations[0]),
        send_observation(socket_b, tick, &observations[1]),
    );
    (result_a.is_ok(), result_b.is_ok())
}

async fn send_observation(socket: &mut WebSocket, tick: u32, observation: &Observation) -> Result<(), ()> {
    let message = protocol::ServerMessage::Observation(protocol::ObservationMessage {
        tick,
        observation: observation.clone(),
    });
    let bytes = protocol::encode(&message).map_err(|_| ())?;
    socket.send(Message::Binary(bytes)).await.map_err(|_| ())
}

/// Envoie `MatchStart` aux deux équipes en parallèle, avant tout le reste
/// (pas encore de `tick_period`/`Engine` construits côté appelant à ce
/// stade : ce message ne dépend que de `match_id`).
async fn send_match_start(
    socket_a: &mut WebSocket,
    socket_b: &mut WebSocket,
    match_id: MatchId,
) -> Result<(), ()> {
    let message = protocol::ServerMessage::MatchStart(protocol::MatchStart {
        match_id: match_id.to_string(),
    });
    let bytes = protocol::encode(&message).map_err(|_| ())?;

    let (result_a, result_b) = tokio::join!(
        socket_a.send(Message::Binary(bytes.clone())),
        socket_b.send(Message::Binary(bytes)),
    );
    result_a.map_err(|_| ())?;
    result_b.map_err(|_| ())
}

async fn send_match_end(
    socket: &mut WebSocket,
    outcome: protocol::MatchOutcome,
    final_score: (u32, u32),
) -> Result<(), ()> {
    let message = protocol::ServerMessage::MatchEnd(protocol::MatchEnd {
        outcome,
        final_score,
    });
    let bytes = protocol::encode(&message).map_err(|_| ())?;
    socket.send(Message::Binary(bytes)).await.map_err(|_| ())
}

/// Déduit le résultat du match pour UNE équipe à partir de son propre score
/// (`Observation::score` est déjà `(soi, adversaire)`, donc cette fonction
/// s'applique identiquement aux deux équipes).
fn outcome_from_score(score: (u32, u32)) -> protocol::MatchOutcome {
    use std::cmp::Ordering;
    match score.0.cmp(&score.1) {
        Ordering::Greater => protocol::MatchOutcome::Win,
        Ordering::Less => protocol::MatchOutcome::Loss,
        Ordering::Equal => protocol::MatchOutcome::Draw,
    }
}

/// Un agent s'est déconnecté en cours de match (détecté soit à la réception
/// d'une action, soit à l'envoi d'une observation — cf. les deux points
/// d'appel dans `run_match`) : celui qui reste (s'il reste bien connecté)
/// gagne par forfait, l'autre... n'est de toute façon plus là pour recevoir
/// quoi que ce soit. Pas de reconnexion en V1 (cf. cahier des charges) : le
/// match se termine ici, sans tenter de rattraper la connexion perdue.
async fn handle_forfeit(
    mut socket_a: WebSocket,
    mut socket_b: WebSocket,
    a_disconnected: bool,
    b_disconnected: bool,
    last_observations: &[Observation; 2],
    context: &MatchContext,
    replay_frames: &[SpectatorFrame],
) {
    tracing::info!(a_disconnected, b_disconnected, "fin de match par forfait");

    if !a_disconnected {
        let _ = send_match_end(
            &mut socket_a,
            protocol::MatchOutcome::OpponentForfeit,
            last_observations[0].score,
        )
        .await;
        let _ = socket_a.close().await;
    }
    if !b_disconnected {
        let _ = send_match_end(
            &mut socket_b,
            protocol::MatchOutcome::OpponentForfeit,
            last_observations[1].score,
        )
        .await;
        let _ = socket_b.close().await;
    }

    // "home" = équipe A = indice 0 : un forfait de son côté est une défaite
    // pour elle (`Loss`), un forfait de l'adversaire est une victoire
    // (`Win`). Si les deux se sont déconnectés en même temps (rare), on
    // enregistre arbitrairement un nul plutôt que de ne rien enregistrer.
    let home_outcome = match (a_disconnected, b_disconnected) {
        (true, true) => protocol::MatchOutcome::Draw,
        (true, false) => protocol::MatchOutcome::Loss,
        (false, true) => protocol::MatchOutcome::Win,
        (false, false) => unreachable!("handle_forfeit appelé sans aucune déconnexion"),
    };
    let (home_score, away_score) = last_observations[0].score;
    context
        .match_store
        .record_end(context.match_id, home_score, away_score, home_outcome, replay_frames)
        .await;
}
