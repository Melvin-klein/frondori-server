//! Module gateway : authentification et matchmaking UNIQUEMENT. Ne contient
//! aucune logique de jeu (elle vit dans l'environnement, exécuté par un
//! worker séparé, cf. `crate::environments`).

pub mod state;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::future::join_all;
use tokio::sync::broadcast;

use crate::environments::EnvInfo;
use crate::gateway::state::{AppState, PendingPlayer};
use crate::match_runner::{run_match, MatchContext, MatchId};

/// Handler axum pour `GET /agent`. `WebSocketUpgrade` négocie le passage de
/// HTTP à WebSocket ; `.on_upgrade(...)` prend une closure appelée une fois
/// la connexion effectivement établie.
pub async fn agent_ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_new_connection(socket, state))
}

/// Traite une nouvelle connexion WebSocket, du handshake au matchmaking.
async fn handle_new_connection(mut socket: WebSocket, state: AppState) {
    // 1. Handshake : le tout premier message doit être un `Hello`.
    let Some(Ok(Message::Binary(bytes))) = socket.recv().await else {
        tracing::debug!("connexion fermée avant tout message");
        return;
    };
    let Ok(protocol::ClientMessage::Hello(hello)) = protocol::decode(&bytes) else {
        tracing::debug!("premier message reçu n'est pas un Hello valide, fermeture");
        let _ = socket.close().await;
        return;
    };

    // 2. Authentification : le token dit QUI joue. L'environnement, lui, est
    //    choisi par le client : un même agent peut jouer à plusieurs jeux.
    let player_id = match state.auth.authenticate(&hello.token).await {
        Ok(player_id) => player_id,
        Err(err) => {
            tracing::debug!(reason = %err.reason, "authentification refusée");
            return reject(socket, err.reason).await;
        }
    };
    let environment = hello.environment;
    let Some(env) = state.catalog.get(&environment) else {
        let mut available: Vec<&String> = state.catalog.keys().collect();
        available.sort();
        let reason = format!("environnement {environment:?} indisponible sur ce serveur (disponibles : {available:?})");
        return reject(socket, reason).await;
    };

    let welcome = protocol::ServerMessage::Welcome(protocol::Welcome {
        player_id: player_id.clone(),
        environment: environment.clone(),
    });
    let Ok(bytes) = protocol::encode(&welcome) else {
        tracing::error!("échec d'encodage du message Welcome");
        return;
    };
    if socket.send(Message::Binary(bytes)).await.is_err() {
        tracing::debug!(%player_id, "déconnexion juste après l'authentification");
        return;
    }

    // 3. Matchmaking FIFO, dans la file de CET environnement, jusqu'à avoir
    //    autant de participants que l'environnement a d'agents.
    let seats = env.agents.len();
    let mut me = Some(PendingPlayer { id: player_id, socket });
    loop {
        // Le verrou est pris et relâché dans ce bloc, JAMAIS à cheval sur un
        // `.await` (cf. doc d'`AppState`).
        let waiting: Vec<PendingPlayer> = {
            let mut queues = state.matchmaking_queues.lock().unwrap();
            let queue = queues.entry(environment.clone()).or_default();
            if queue.len() + 1 < seats {
                // Pas encore assez de monde : on attend en file, et c'est un
                // prochain arrivant qui lancera le match.
                queue.push_back(me.take().expect("présent tant qu'on n'est ni en file ni en match"));
                return;
            }
            queue.drain(..seats - 1).collect()
        };

        // Un joueur en file peut s'être déconnecté depuis son arrivée : un
        // ping rapide à chacun évite de lancer un match avec un socket mort.
        let checked = join_all(waiting.into_iter().map(|mut player| async move {
            let alive = quick_ping_check(&mut player.socket).await;
            (player, alive)
        }))
        .await;
        let (alive, dead): (Vec<_>, Vec<_>) = checked.into_iter().partition(|(_, alive)| *alive);
        let alive: Vec<PendingPlayer> = alive.into_iter().map(|(player, _)| player).collect();

        if !dead.is_empty() {
            for (player, _) in &dead {
                tracing::warn!(player_id = %player.id, "joueur en file déconnecté, retiré");
            }
            // Les survivants reprennent leur place en tête de file, dans
            // leur ordre d'arrivée, puis on retente.
            let mut queues = state.matchmaking_queues.lock().unwrap();
            let queue = queues.entry(environment.clone()).or_default();
            for player in alive.into_iter().rev() {
                queue.push_front(player);
            }
            continue;
        }

        let mut players = alive;
        players.push(me.take().expect("présent tant qu'on n'est ni en file ni en match"));
        start_match(&state, environment, env.clone(), players);
        return;
    }
}

/// Crée le match : identifiant, canal spectateur, puis lance sa boucle dans
/// une tâche tokio INDÉPENDANTE, qui continue de vivre après le retour de
/// cette fonction.
fn start_match(state: &AppState, environment: String, env: EnvInfo, players: Vec<PendingPlayer>) {
    let match_id = MatchId::new_v4();
    let player_ids: Vec<&str> = players.iter().map(|p| p.id.as_str()).collect();
    tracing::info!(%match_id, %environment, players = ?player_ids, "appariement, démarrage du match");

    // Canal de diffusion pour les spectateurs de CE match. Capacité 64 : un
    // spectateur temporairement lent saute les scènes les plus anciennes
    // plutôt que de ralentir le match (cf. `stream_spectator_frames`).
    let (spectator_tx, _) = broadcast::channel(64);
    state.spectators.lock().unwrap().insert(match_id, spectator_tx.clone());

    // C'est le gateway, pas `run_match`, qui retire le match du registre à
    // sa fin : le registre ne grossit pas indéfiniment.
    let spectators = state.spectators.clone();
    let context = MatchContext {
        match_id,
        environment,
        env,
        worker_command: state.worker_command.clone(),
        match_store: state.match_store.clone(),
        spectator_tx,
    };
    let config = state.match_config.clone();
    tokio::spawn(async move {
        run_match(players, config, rand::random(), context).await;
        spectators.lock().unwrap().remove(&match_id);
    });
}

async fn reject(mut socket: WebSocket, reason: String) {
    let message = protocol::ServerMessage::AuthError(protocol::AuthError { reason });
    if let Ok(bytes) = protocol::encode(&message) {
        let _ = socket.send(Message::Binary(bytes)).await;
    }
    let _ = socket.close().await;
}

/// Vérifie qu'un socket en attente dans la file est toujours valide : on lui
/// envoie un `Ping` protocolaire et on attend un `Pong` en retour, avec un
/// délai court. Pas de réponse (ou erreur) => joueur considéré déconnecté.
async fn quick_ping_check(socket: &mut WebSocket) -> bool {
    const PING_TIMEOUT: Duration = Duration::from_millis(500);

    let ping = protocol::ServerMessage::Ping(protocol::Ping { nonce: 0 });
    let Ok(bytes) = protocol::encode(&ping) else {
        return false;
    };
    if socket.send(Message::Binary(bytes)).await.is_err() {
        return false;
    }

    let Ok(Some(Ok(Message::Binary(bytes)))) = tokio::time::timeout(PING_TIMEOUT, socket.recv()).await else {
        return false;
    };

    matches!(
        protocol::decode::<protocol::ClientMessage>(&bytes),
        Ok(protocol::ClientMessage::Pong(_))
    )
}

/// Handler axum pour `GET /spectate/:match_id` : diffuse en direct la scène
/// d'un match EN COURS (format générique, cf. `frondori_engine/scene.py`),
/// en JSON texte — directement consommable par un navigateur. Aucune
/// authentification : ce flux est public par conception.
pub async fn spectate_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(match_id): Path<String>,
) -> Response {
    let Ok(match_id) = match_id.parse::<MatchId>() else {
        return (StatusCode::BAD_REQUEST, "identifiant de match invalide").into_response();
    };

    let receiver = state
        .spectators
        .lock()
        .unwrap()
        .get(&match_id)
        .map(|sender| sender.subscribe());

    let Some(receiver) = receiver else {
        return (
            StatusCode::NOT_FOUND,
            "aucun match en direct avec cet identifiant (terminé, ou jamais existé)",
        )
            .into_response();
    };

    ws.on_upgrade(move |socket| stream_spectator_frames(socket, receiver))
}

/// Relaie les scènes du canal de diffusion vers le socket du spectateur,
/// jusqu'à ce que l'une des deux parties se ferme.
async fn stream_spectator_frames(mut socket: WebSocket, mut receiver: broadcast::Receiver<Arc<str>>) {
    loop {
        match receiver.recv().await {
            Ok(scene) => {
                if socket.send(Message::Text(scene.to_string())).await.is_err() {
                    return; // spectateur déconnecté
                }
            }
            // Spectateur trop lent pour suivre : on saute aux scènes
            // suivantes plutôt que de couper la connexion.
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            // Plus aucun `Sender` : le match est terminé.
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}
