//! Les actions sont exprimées dans le MÊME repère que les observations :
//! pour chaque équipe, +x veut dire "vers le but adverse", quel que soit son
//! côté réel du terrain. C'est ce qui permet à un même modèle de jouer des
//! deux côtés.
//!
//! Régression d'un bug réel : l'observation de l'équipe 1 était bien
//! mirrorée, mais pas ses actions. Une politique "j'avance et je tire vers
//! +x" marquait donc contre son camp dès qu'elle jouait en équipe 1 (trouvé
//! en faisant jouer la même politique aux deux équipes en réseau : 113-0).

use engine::config::EngineConfig;
use engine::types::{Action, Actions};
use engine::Engine;

fn one_player(action: Action) -> Actions {
    Actions { players: vec![action] }
}

fn small_field() -> EngineConfig {
    // Même petit terrain que `goal_detection.rs` : un but y est atteignable
    // en quelques dizaines de ticks.
    EngineConfig {
        players_per_team: 1,
        field_width: 10.0,
        field_height: 6.0,
        goal_width: 3.0,
        max_ticks: 200,
        ..EngineConfig::default()
    }
}

#[test]
fn moving_forward_brings_each_team_toward_the_opponent_goal() {
    let mut engine = Engine::new(small_field(), 1);
    engine.reset();

    // Au coup d'envoi, les deux joueurs sont face à face de part et d'autre
    // du ballon : en avançant tout de suite, ils se percuteraient. On les
    // écarte d'abord sur deux couloirs différents (l'axe y n'est jamais
    // mirroré).
    let apart = |dy: f32| one_player(Action { move_dir: (0.0, dy), kick: None });
    for _ in 0..10 {
        engine.step([apart(1.0), apart(-1.0)]);
    }
    let before = engine.spectator_frame();

    let forward = one_player(Action {
        move_dir: (1.0, 0.0),
        kick: None,
    });
    let mut result = engine.step([forward.clone(), forward.clone()]);
    for _ in 0..5 {
        result = engine.step([forward.clone(), forward.clone()]);
    }
    let after = engine.spectator_frame();

    // Dans le repère RÉEL du terrain : l'équipe 0 attaque vers +x,
    // l'équipe 1 vers -x.
    assert!(after.players[0].position.0 > before.players[0].position.0);
    assert!(after.players[1].position.0 < before.players[1].position.0);
    // Dans leur PROPRE repère (leur observation), toutes deux ont avancé.
    assert!(result.observations[0].self_team[0].velocity.0 > 0.0);
    assert!(result.observations[1].self_team[0].velocity.0 > 0.0);
}

#[test]
fn a_forward_shot_by_team_1_scores_for_team_1() {
    // Miroir exact de `goal_detection.rs` : cette fois c'est l'équipe 0 qui
    // dégage le couloir, et l'équipe 1 qui tire "vers l'avant" (+x dans son
    // repère).
    let mut engine = Engine::new(small_field(), 7);
    engine.reset();
    let noop = one_player(Action::NOOP);
    let move_away = one_player(Action {
        move_dir: (0.0, 1.0),
        kick: None,
    });
    for _ in 0..15 {
        engine.step([move_away.clone(), noop.clone()]);
    }

    let shot = one_player(Action {
        move_dir: (0.0, 0.0),
        kick: Some((1.0, 0.0)),
    });
    let mut result = engine.step([noop.clone(), shot]);
    let mut ticks = 1;
    while result.observations[1].score == (0, 0) && ticks < 90 {
        result = engine.step([noop.clone(), noop.clone()]);
        ticks += 1;
    }

    // Score vu par l'équipe 1 : (marqués, encaissés).
    assert_eq!(result.observations[1].score, (1, 0), "après {ticks} ticks");
    assert_eq!(result.rewards[1].goal_scored, 1.0);
}
