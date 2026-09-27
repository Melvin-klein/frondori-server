//! Test de stabilité numérique : sur beaucoup de ticks d'actions no-op,
//! aucune position/vitesse ne doit diverger (NaN, infini, ou valeurs
//! aberrantes dues à une instabilité du solveur physique).

use engine::config::EngineConfig;
use engine::types::{Action, Actions};
use engine::Engine;

#[test]
fn noop_actions_do_not_produce_nan_or_explosive_velocities() {
    let config = EngineConfig::default();
    let n = config.players_per_team;
    let mut engine = Engine::new(config, /* seed */ 99);
    engine.reset();

    let noop = Actions {
        players: vec![Action::NOOP; n],
    };

    for _ in 0..10_000 {
        let result = engine.step([noop.clone(), noop.clone()]);

        for team_obs in &result.observations {
            for p in team_obs.self_team.iter().chain(team_obs.opponent_team.iter()) {
                assert!(p.position.0.is_finite() && p.position.1.is_finite());
                assert!(p.velocity.0.is_finite() && p.velocity.1.is_finite());
                // Vitesse normalisée : une divergence numérique se traduirait
                // typiquement par une vitesse qui explose bien au-delà de ce
                // qui est physiquement plausible sur un terrain normalisé.
                assert!(p.velocity.0.abs() < 100.0 && p.velocity.1.abs() < 100.0);
            }
            assert!(team_obs.ball.position.0.is_finite() && team_obs.ball.position.1.is_finite());
        }

        if result.done {
            break;
        }
    }
}
