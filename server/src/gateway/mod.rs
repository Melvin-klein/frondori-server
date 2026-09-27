//! Module gateway : authentification et matchmaking UNIQUEMENT. Ne doit
//! jamais contenir de logique de simulation (celle-ci vit dans `match_runner`
//! et `engine`).

pub mod state;

use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use engine::types::SpectatorFrame;
use tokio::sync::broadcast;

use crate::gateway::state::{AppState, PendingPlayer};
use crate::match_runner::{run_match, MatchContext, MatchId};

/// Handler axum pour `GET /agent`. `WebSocketUpgrade` négocie le passage de
/// HTTP à WebSocket ; `.on_upgrade(...)` prend une closure appelée une fois
/// la connexion effectivement établie, avec le `WebSocket` déjà prêt à
/// envoyer/recevoir des messages.
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

    // 2. Authentification, via l'abstraction `AuthProvider` (implémentation
    // en mémoire pour cette V1, remplaçable par une vraie base plus tard
    // sans toucher à ce module).
    let player_id = match state.auth.authenticate(&hello.token).await {
        Ok(id) => id,
        Err(err) => {
            tracing::debug!(reason = %err.reason, "authentification refusée");
            let message = protocol::ServerMessage::AuthError(protocol::AuthError {
                reason: err.reason,
            });
            if let Ok(bytes) = protocol::encode(&message) {
                let _ = socket.send(Message::Binary(bytes)).await;
            }
            let _ = socket.close().await;
            return;
        }
    };

    let welcome = protocol::ServerMessage::Welcome(protocol::Welcome {
        player_id: player_id.clone(),
    });
    let Ok(bytes) = protocol::encode(&welcome) else {
        tracing::error!("échec d'encodage du message Welcome");
        return;
    };
    if socket.send(Message::Binary(bytes)).await.is_err() {
        tracing::debug!(%player_id, "déconnexion juste après l'authentification");
        return;
    }

    // 3. Matchmaking FIFO.
    //
    // `state.matchmaking_queue.lock().unwrap()` : le verrou est pris et
    // relâché dans le même bloc `{ ... }`, JAMAIS à cheval sur un `.await`
    // (cf. doc d'`AppState` : c'est précisément ce qui permet d'utiliser un
    // `std::sync::Mutex`, plus léger qu'un mutex async, sans risquer de
    // bloquer tout le runtime tokio).
    loop {
        let waiting_opponent = {
            let mut queue = state.matchmaking_queue.lock().unwrap();
            queue.pop_front()
        };

        match waiting_opponent {
            None => {
                // Personne n'attend : on se met en file, et c'est le
                // PROCHAIN arrivant qui déclenchera l'appariement.
                let mut queue = state.matchmaking_queue.lock().unwrap();
                queue.push_back(PendingPlayer {
                    id: player_id,
                    socket,
                });
                return;
            }
            Some(mut opponent) => {
                // Le joueur en attente peut s'être déconnecté entre le
                // moment où il a rejoint la file et maintenant : un ping
                // rapide avant de démarrer le match évite de spawn un match
                // avec un socket déjà mort.
                if !quick_ping_check(&mut opponent.socket).await {
                    tracing::warn!(
                        player_id = %opponent.id,
                        "joueur en file déconnecté, retiré ; on retente avec le suivant"
                    );
                    continue; // reboucle : retente avec le prochain de la file (ou se met en file si elle est vide)
                }

                let match_id = MatchId::new_v4();
                tracing::info!(a = %opponent.id, b = %player_id, %match_id, "appariement, démarrage du match");

                // Canal de diffusion pour les spectateurs de CE match :
                // `broadcast::channel` retourne un `Sender` et un
                // `Receiver` initial ; on jette ce premier `Receiver` (aucun
                // spectateur n'est encore connecté), les suivants viendront
                // de `sender.subscribe()` dans `spectate_ws_handler`. La
                // capacité (64) borne le nombre de frames gardées en
                // mémoire pour un spectateur temporairement lent ; au-delà,
                // il saute les plus anciennes plutôt que de tout bloquer
                // (cf. `spectate_ws_handler`).
                let (spectator_tx, _) = broadcast::channel(64);
                state
                    .spectators
                    .lock()
                    .unwrap()
                    .insert(match_id, spectator_tx.clone());

                // `state.spectators` (Arc) cloné pour la tâche de nettoyage
                // ci-dessous : `run_match` ne connaît rien du registre, il
                // se contente d'émettre sur `spectator_tx`. C'est le gateway
                // qui retire l'entrée une fois le match terminé, pour ne pas
                // laisser le registre grossir indéfiniment au fil des matchs.
                let spectators = state.spectators.clone();
                let context = MatchContext {
                    match_id,
                    home_player_id: opponent.id,
                    away_player_id: player_id,
                    match_store: state.match_store.clone(),
                    spectator_tx,
                };
                // `tokio::spawn` : lance cette tâche comme une tâche tokio
                // INDÉPENDANTE, qui continue de vivre même après que cette
                // fonction (`handle_new_connection`) se termine. Les deux
                // sockets sont déplacés (move) dans la closure/l'appel :
                // cette fonction-ci n'y touche plus jamais après.
                tokio::spawn(async move {
                    run_match(
                        opponent.socket,
                        socket,
                        state.match_config.clone(),
                        state.engine_config.clone(),
                        rand::random(),
                        context,
                    )
                    .await;
                    spectators.lock().unwrap().remove(&match_id);
                });
                return;
            }
        }
    }
}

/// Vérifie qu'un socket en attente dans la file est toujours valide : on lui
/// envoie un `Ping` protocolaire et on attend un `Pong` en retour, avec un
/// délai court. Pas de réponse (ou erreur) => on considère le joueur
/// déconnecté.
async fn quick_ping_check(socket: &mut WebSocket) -> bool {
    const PING_TIMEOUT: Duration = Duration::from_millis(500);

    let ping = protocol::ServerMessage::Ping(protocol::Ping { nonce: 0 });
    let Ok(bytes) = protocol::encode(&ping) else {
        return false;
    };
    if socket.send(Message::Binary(bytes)).await.is_err() {
        return false;
    }

    let Ok(Some(Ok(Message::Binary(bytes)))) =
        tokio::time::timeout(PING_TIMEOUT, socket.recv()).await
    else {
        return false;
    };

    matches!(
        protocol::decode::<protocol::ClientMessage>(&bytes),
        Ok(protocol::ClientMessage::Pong(_))
    )
}

/// Handler axum pour `GET /spectate/:match_id` : diffuse en direct l'état
/// (absolu, jamais mirroré) d'un match EN COURS à qui se connecte, en JSON
/// (pas MessagePack — ce flux n'a pas la même contrainte de performance que
/// le protocole compétitif, et JSON se consomme nativement dans un
/// navigateur). Aucune authentification : ce flux est public par conception
/// (l'objectif est justement que n'importe qui puisse regarder).
pub async fn spectate_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(match_id): Path<String>,
) -> Response {
    let Ok(match_id) = match_id.parse::<MatchId>() else {
        return (StatusCode::BAD_REQUEST, "identifiant de match invalide").into_response();
    };

    // Le verrou n'est tenu que le temps de cette ligne (`.subscribe()` est
    // synchrone, pas d'`.await`) : même contrainte que partout ailleurs sur
    // `std::sync::Mutex` dans ce module.
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

/// Relaie les frames du canal de diffusion vers le socket du spectateur,
/// jusqu'à ce que l'une des deux parties se ferme.
async fn stream_spectator_frames(mut socket: WebSocket, mut receiver: broadcast::Receiver<SpectatorFrame>) {
    loop {
        match receiver.recv().await {
            Ok(frame) => {
                let Ok(json) = serde_json::to_string(&frame) else {
                    continue; // ne devrait jamais arriver (SpectatorFrame est toujours sérialisable)
                };
                if socket.send(Message::Text(json)).await.is_err() {
                    return; // spectateur déconnecté
                }
            }
            // Le spectateur était trop lent pour suivre le rythme des
            // frames (canal de capacité 64, cf. `handle_new_connection`) :
            // on saute directement aux frames suivantes plutôt que de
            // couper la connexion pour un simple retard.
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            // Plus aucun `Sender` actif : le match est terminé (le gateway
            // retire l'entrée du registre après `run_match`, ce qui fait
            // tomber le dernier `Sender` connu). Rien à faire de plus, on
            // ferme proprement en sortant de la boucle.
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}
