//! Utilitaires partagés entre les tests d'intégration de `server`.
//!
//! `tests/common/mod.rs` (un sous-dossier avec un `mod.rs`, pas
//! `tests/common.rs`) : c'est la convention Cargo pour qu'un fichier de
//! `tests/` ne devienne PAS lui-même un binaire de test à part entière (qui
//! afficherait "running 0 tests" à chaque run, pour rien) — Cargo compile
//! chaque fichier DIRECTEMENT sous `tests/` comme un crate de test séparé,
//! mais ignore les sous-dossiers.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use engine::config::EngineConfig;
use engine::types::{Action, Actions};
use server::auth::InMemoryAuthProvider;
use server::match_runner::MatchRunnerConfig;
use server::matches::NullMatchStore;

/// Démarre le serveur (gateway + match runner) sur un port TCP choisi par
/// l'OS (`127.0.0.1:0`), avec deux tokens de test pré-enregistrés
/// (`token-a` -> `player-a`, `token-b` -> `player-b`) et la persistance des
/// matchs désactivée (`NullMatchStore` — cf. `matches_persistence_test.rs`
/// pour un test qui exerce spécifiquement la persistance Postgres).
/// Retourne l'adresse effective sur laquelle il écoute.
///
/// `#[allow(dead_code)]` : chaque fichier `tests/*.rs` qui fait `mod common;`
/// recompile sa PROPRE copie de ce module (chaque fichier d'intégration est
/// un crate de test séparé) ; certains n'utilisent que `play_dummy_match`,
/// pas cette fonction, ce qui déclencherait sinon un faux positif de
/// `dead_code` selon le fichier compilé.
#[allow(dead_code)]
pub async fn start_test_server(engine_config: EngineConfig) -> std::net::SocketAddr {
    let mut tokens = HashMap::new();
    tokens.insert("token-a".to_string(), "player-a".to_string());
    tokens.insert("token-b".to_string(), "player-b".to_string());
    let auth = Arc::new(InMemoryAuthProvider::new(tokens));

    let state = server::new_app_state(
        auth,
        Arc::new(NullMatchStore),
        engine_config,
        MatchRunnerConfig::default(),
    );
    let app = server::router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("impossible de binder un port de test");
    let addr = listener.local_addr().unwrap();

    // Tâche tokio indépendante, comme en production : ce test s'arrête tout
    // seul à la fin de la fonction de test, la tâche du serveur est alors
    // simplement abandonnée (pas de `shutdown` propre nécessaire ici).
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    addr
}

/// Joue un match complet du point de vue d'un client SDK minimal : se
/// connecte, s'authentifie, répond `NOOP` à chaque observation reçue, et
/// répond aux `Ping` de latence, jusqu'à recevoir le `MatchEnd`.
///
/// `match_id_tx`, si fourni, reçoit l'identifiant public du match DÈS qu'il
/// est connu (message `MatchStart`, tout début du match) — utile pour un
/// test qui doit agir PENDANT que le match tourne encore (ex: se connecter
/// en spectateur), pas seulement une fois le `MatchEnd` reçu.
pub async fn play_dummy_match(
    url: String,
    token: &str,
    mut match_id_tx: Option<tokio::sync::oneshot::Sender<String>>,
) -> protocol::MatchEnd {
    let (mut ws, _response) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connexion WebSocket échouée");

    let hello = protocol::ClientMessage::Hello(protocol::Hello {
        token: token.to_string(),
        client_name: "smoke-test-client".to_string(),
    });
    ws.send(WsMessage::Binary(protocol::encode(&hello).unwrap()))
        .await
        .expect("envoi du Hello échoué");

    // Handshake : on attend un Welcome (un AuthError ferait échouer le test
    // explicitement, plutôt que de bloquer indéfiniment).
    loop {
        let Some(Ok(WsMessage::Binary(bytes))) = ws.next().await else {
            panic!("connexion fermée avant réception du Welcome");
        };
        match protocol::decode::<protocol::ServerMessage>(&bytes).unwrap() {
            protocol::ServerMessage::Welcome(_) => break,
            protocol::ServerMessage::AuthError(err) => {
                panic!("authentification refusée: {}", err.reason)
            }
            other => panic!("message inattendu avant le Welcome: {other:?}"),
        }
    }

    // Boucle de match : MatchStart (une fois) ; Ping (latence) -> Pong ;
    // Observation -> Action (NOOP) ; MatchEnd -> on retourne le résultat.
    loop {
        let Some(Ok(WsMessage::Binary(bytes))) = ws.next().await else {
            panic!("connexion fermée en cours de match");
        };
        match protocol::decode::<protocol::ServerMessage>(&bytes).unwrap() {
            protocol::ServerMessage::MatchStart(start) => {
                if let Some(tx) = match_id_tx.take() {
                    let _ = tx.send(start.match_id);
                }
            }
            protocol::ServerMessage::Ping(ping) => {
                let pong = protocol::ClientMessage::Pong(protocol::Pong { nonce: ping.nonce });
                ws.send(WsMessage::Binary(protocol::encode(&pong).unwrap()))
                    .await
                    .expect("envoi du Pong échoué");
            }
            protocol::ServerMessage::Observation(obs_msg) => {
                let n = obs_msg.observation.self_team.len();
                let action = protocol::ClientMessage::Action(protocol::ActionMessage {
                    tick: obs_msg.tick,
                    actions: Actions {
                        players: vec![Action::NOOP; n],
                    },
                });
                ws.send(WsMessage::Binary(protocol::encode(&action).unwrap()))
                    .await
                    .expect("envoi de l'action échoué");
            }
            protocol::ServerMessage::MatchEnd(end) => return end,
            other => panic!("message inattendu pendant le match: {other:?}"),
        }
    }
}
