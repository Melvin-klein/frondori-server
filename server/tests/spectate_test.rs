//! Test de bout en bout de l'endpoint spectateur (`/spectate/:match_id`) :
//! un vrai match se joue, un spectateur s'y connecte PENDANT qu'il tourne
//! encore, et reçoit la scène générique de l'environnement en JSON.

mod common;

use std::sync::Arc;

use futures_util::StreamExt;
use server::matches::NullMatchStore;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use common::{play_match, Behaviour, Outcome};

#[tokio::test]
async fn spectator_receives_live_scenes_of_an_ongoing_match() {
    // 100 pas par seconde : les 200 pas de la cuisine durent 2 s, largement
    // de quoi laisser le spectateur se connecter en cours de match.
    let addr = common::start_test_server_with(Arc::new(NullMatchStore), 100.0).await;
    let url = format!("ws://{addr}/agent");
    let (match_id_tx, match_id_rx) = tokio::sync::oneshot::channel();

    let first = Behaviour {
        match_id_tx: Some(match_id_tx),
        ..Behaviour::default()
    };
    let players = tokio::spawn(async move {
        tokio::join!(
            play_match(url.clone(), "token-a", "kitchen-v0", first),
            play_match(url, "token-b", "kitchen-v0", Behaviour::default()),
        )
    });

    let match_id = match_id_rx.await.expect("le match_id n'a jamais été reçu");
    let (mut spectator, _response) = tokio_tungstenite::connect_async(format!("ws://{addr}/spectate/{match_id}"))
        .await
        .expect("connexion spectateur échouée");

    let message = spectator
        .next()
        .await
        .expect("le spectateur n'a reçu aucun message")
        .expect("erreur websocket côté spectateur");
    let WsMessage::Text(text) = message else {
        panic!("message spectateur inattendu (pas du texte JSON) : {message:?}");
    };
    let scene: serde_json::Value = serde_json::from_str(&text).expect("la scène n'est pas du JSON valide");
    assert!(scene["width"].as_f64().is_some_and(|w| w > 0.0));
    assert!(scene["shapes"].as_array().is_some_and(|shapes| !shapes.is_empty()));

    let (a, b) = players.await.expect("la tâche des joueurs a paniqué");
    assert!(matches!(a, Outcome::Finished(_)));
    assert!(matches!(b, Outcome::Finished(_)));
}

#[tokio::test]
async fn spectate_returns_404_for_unknown_match_id() {
    let addr = common::start_test_server().await;
    let url = format!("ws://{addr}/spectate/00000000-0000-0000-0000-000000000000");

    // Une connexion vers un match inexistant doit échouer dès le handshake
    // HTTP (404), pas ouvrir un WebSocket qui ne recevrait jamais rien.
    assert!(tokio_tungstenite::connect_async(url).await.is_err());
}
