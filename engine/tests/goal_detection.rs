//! Test de détection de but : un tir cadré doit être compté et incrémenter
//! le score de la bonne équipe.

use engine::config::EngineConfig;
use engine::types::{Action, Actions};
use engine::Engine;

#[test]
fn ball_crossing_goal_line_increments_score() {
    // Terrain volontairement petit : on ne teste pas le réalisme physique
    // ici, juste que "ballon au-delà de la ligne, dans le couloir de la
    // cage" incrémente bien le score. Un petit terrain rend le but
    // atteignable en peu de ticks, donc le test reste rapide et fiable (pas
    // besoin d'attendre que le ballon parcoure 20 mètres).
    let config = EngineConfig {
        players_per_team: 1,
        field_width: 10.0,
        field_height: 6.0,
        goal_width: 3.0,
        max_ticks: 200,
        ..EngineConfig::default()
    };

    let mut engine = Engine::new(config, /* seed */ 7);
    let initial = engine.reset();
    assert_eq!(initial[0].score, (0, 0));

    let noop = Actions {
        players: vec![Action::NOOP],
    };

    // Au coup d'envoi, le joueur 0 de CHAQUE équipe est placé juste à côté
    // du ballon (cf. `Engine::formation_slot`), l'un en face de l'autre —
    // comme au centre du terrain sur un vrai coup d'envoi. Si on tire tout
    // de suite, le ballon percute le joueur adverse au lieu de filer vers le
    // but. On fait donc d'abord dégager le joueur de l'équipe B (quelques
    // ticks de déplacement vers le haut du terrain) avant de tirer.
    let move_away = Actions {
        players: vec![Action {
            move_dir: (0.0, 1.0),
            kick: None,
        }],
    };
    for _ in 0..15 {
        engine.step([noop.clone(), move_away.clone()]);
    }

    // Le couloir est dégagé : l'équipe A tire à pleine puissance vers +x,
    // c'est-à-dire vers le but qu'elle attaque.
    let kickoff_shot = Actions {
        players: vec![Action {
            move_dir: (0.0, 0.0),
            kick: Some((1.0, 0.0)),
        }],
    };

    let mut result = engine.step([kickoff_shot, noop.clone()]);

    // On laisse ensuite le ballon filer sans plus aucune action, jusqu'à ce
    // qu'un but soit détecté (ou que le test échoue faute de but dans un
    // délai large, ce qui indiquerait une régression physique plutôt qu'un
    // simple manque de patience du test).
    let mut ticks = 1;
    while result.observations[0].score.0 == 0 && ticks < 90 {
        result = engine.step([noop.clone(), noop.clone()]);
        ticks += 1;
    }

    assert_eq!(
        result.observations[0].score,
        (1, 0),
        "le ballon n'a pas fini par marquer un but après {ticks} ticks"
    );
    // Côté équipe A (qui a marqué) : reward positive ce tick précis.
    assert_eq!(result.rewards[0].goal_scored, 1.0);
    assert_eq!(result.rewards[0].goal_conceded, 0.0);
    // Côté équipe B (qui a encaissé) : reward négative ce tick précis.
    assert_eq!(result.rewards[1].goal_conceded, -1.0);
    assert_eq!(result.rewards[1].goal_scored, 0.0);
}
