//! Test de `Engine::spectator_frame` : contrairement à `Observation`, ce
//! type ne doit JAMAIS mirrorer les coordonnées — un spectateur regarde le
//! vrai terrain, pas une vue relative à une équipe.

use engine::config::EngineConfig;
use engine::Engine;

#[test]
fn spectator_frame_uses_absolute_non_mirrored_coordinates() {
    let config = EngineConfig {
        players_per_team: 2,
        ..EngineConfig::default()
    };
    let mut engine = Engine::new(config, /* seed */ 1);
    engine.reset();

    let frame = engine.spectator_frame();

    assert_eq!(frame.tick, 0);
    assert_eq!(frame.score, (0, 0));
    assert_eq!(frame.players.len(), 4);

    // Équipe 0 défend x = -1 : au coup d'envoi, ses joueurs sont donc du
    // côté négatif du terrain. Équipe 1, symétriquement, du côté positif.
    // C'est cette asymétrie (jamais retournée) qui distingue ce type
    // d'`Observation`, où l'équipe 1 verrait au contraire SES joueurs comme
    // étant du côté "négatif" par mirroring.
    for player in &frame.players {
        match player.team {
            0 => assert!(
                player.position.0 < 0.0,
                "un joueur de l'équipe 0 devrait être du côté négatif du terrain, position={:?}",
                player.position
            ),
            1 => assert!(
                player.position.0 > 0.0,
                "un joueur de l'équipe 1 devrait être du côté positif du terrain, position={:?}",
                player.position
            ),
            other => panic!("équipe inattendue: {other}"),
        }
    }

    // Le ballon démarre au centre exact du terrain.
    assert_eq!(frame.ball.position, (0.0, 0.0));
}
