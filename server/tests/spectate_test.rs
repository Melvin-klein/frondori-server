//! Test de bout en bout de l'endpoint spectateur (`/spectate/:match_id`) :
//! démarre un vrai serveur, fait jouer un vrai match entre deux clients
//! (comme `smoke_test.rs`), récupère le `match_id` via le message
//! `MatchStart`, puis se connecte en spectateur PENDANT que le match tourne
//! encore, et vérifie que de vraies frames JSON arrivent.

mod common;

use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use engine::config::EngineConfig;

#[tokio::test]
async fn spectator_receives_live_json_frames_for_an_ongoing_match() {
    // Assez de ticks pour laisser le temps au spectateur de se connecter
    // avant la fin du match (le `match_id` arrive dès le tout premier
    // message, mais la connexion spectateur elle-même prend un peu de
    // temps réseau).
    let engine_config = EngineConfig {
        players_per_team: 1,
        max_ticks: 30,
        ..EngineConfig::default()
    };
    let addr = common::start_test_server(engine_config).await;
    let agent_url = format!("ws://{addr}/agent");

    let (match_id_tx, match_id_rx) = tokio::sync::oneshot::channel();

    // Les deux joueurs tournent dans une tâche séparée : ce test a besoin de
    // continuer à faire autre chose (se connecter en spectateur) PENDANT
    // que le match se joue, pas seulement une fois qu'il est terminé.
    let players = tokio::spawn(async move {
        tokio::join!(
            common::play_dummy_match(agent_url.clone(), "token-a", Some(match_id_tx)),
            common::play_dummy_match(agent_url, "token-b", None),
        )
    });

    let match_id = match_id_rx
        .await
        .expect("le match_id n'a jamais été reçu (MatchStart manquant ?)");

    let spectate_url = format!("ws://{addr}/spectate/{match_id}");
    let (mut spectator_ws, _response) = tokio_tungstenite::connect_async(spectate_url)
        .await
        .expect("connexion spectateur échouée");

    let message = spectator_ws
        .next()
        .await
        .expect("le spectateur n'a reçu aucun message")
        .expect("erreur websocket côté spectateur");
    let WsMessage::Text(text) = message else {
        panic!("frame spectateur inattendue (pas du texte JSON): {message:?}");
    };

    // On ne redéfinit pas de types dédiés côté test : `serde_json::Value`
    // suffit à vérifier la FORME du message (c'est un objet JSON avec les
    // bons champs), sans dupliquer `SpectatorFrame`.
    let frame: serde_json::Value =
        serde_json::from_str(&text).expect("la frame spectateur n'est pas du JSON valide");
    assert!(frame.get("tick").is_some());
    assert!(frame.get("players").and_then(|p| p.as_array()).is_some());
    assert!(frame.get("ball").is_some());
    assert!(frame.get("score").is_some());

    // Laisse les deux matchs se terminer proprement avant la fin du test
    // (sinon la tâche spawnée serait juste abandonnée en plein milieu).
    let (end_a, end_b) = players.await.expect("la tâche des joueurs a paniqué");
    assert_eq!(end_a.outcome, protocol::MatchOutcome::Draw);
    assert_eq!(end_b.outcome, protocol::MatchOutcome::Draw);
}

#[tokio::test]
async fn spectate_returns_404_for_unknown_match_id() {
    let addr = common::start_test_server(EngineConfig::default()).await;
    let url = format!("ws://{addr}/spectate/00000000-0000-0000-0000-000000000000");

    // Une connexion WS vers un match inexistant doit échouer proprement au
    // moment du handshake HTTP (404), pas se traduire par une connexion
    // WebSocket qui ne reçoit ensuite jamais rien.
    let result = tokio_tungstenite::connect_async(url).await;
    assert!(
        result.is_err(),
        "la connexion aurait dû échouer (404), pas réussir"
    );
}
