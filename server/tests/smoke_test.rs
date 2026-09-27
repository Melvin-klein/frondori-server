//! Test de bout en bout : démarre un vrai serveur (vrai port TCP), y
//! connecte deux VRAIS clients WebSocket (`tokio-tungstenite`, pas
//! `axum::extract::ws` — on veut jouer le rôle d'un SDK participant, pas
//! réutiliser la même implémentation que le serveur), et joue un match
//! complet du handshake jusqu'au `MatchEnd`.
//!
//! C'est le seul test qui aurait attrapé, par exemple, un bug de framing
//! MessagePack ou un blocage dans le handshake de matchmaking : les tests
//! du crate `engine` valident la physique, mais jamais le réseau.

mod common;

use engine::config::EngineConfig;

#[tokio::test]
async fn two_clients_play_a_full_match_end_to_end() {
    // Match volontairement très court (peu de ticks) pour que le test reste
    // rapide, indépendamment de la durée par défaut (5 minutes) utilisée en
    // production.
    let engine_config = EngineConfig {
        players_per_team: 1,
        max_ticks: 5,
        ..EngineConfig::default()
    };
    let addr = common::start_test_server(engine_config).await;
    let url = format!("ws://{addr}/agent");

    // Les deux clients jouent en parallèle : c'est indispensable, le
    // matchmaking FIFO n'apparie ces deux joueurs qu'au moment où le second
    // se connecte pendant que le premier attend déjà dans la file.
    let (end_a, end_b) = tokio::join!(
        common::play_dummy_match(url.clone(), "token-a", None),
        common::play_dummy_match(url, "token-b", None),
    );

    // Aucun des deux ne tire jamais (actions NOOP) : le match se termine
    // forcément nul, 0-0.
    assert_eq!(end_a.outcome, protocol::MatchOutcome::Draw);
    assert_eq!(end_b.outcome, protocol::MatchOutcome::Draw);
    assert_eq!(end_a.final_score, (0, 0));
    assert_eq!(end_b.final_score, (0, 0));
}
