//! Test de déterminisme : le contrat le plus important du moteur.
//!
//! Fichiers dans `tests/` = tests d'intégration : chacun est compilé comme un
//! crate séparé qui dépend d'`engine` en tant que librairie externe (via
//! `use engine::...`), contrairement aux tests unitaires qu'on mettrait dans
//! `#[cfg(test)] mod tests { ... }` à l'intérieur de `src/`.

use engine::config::EngineConfig;
use engine::types::{Action, Actions};
use engine::Engine;

fn fixed_actions(n_players: usize) -> Actions {
    // Séquence d'actions non-aléatoire, identique à chaque appel : nécessaire
    // pour isoler le test du déterminisme du moteur (si on utilisait
    // `rand::thread_rng()` ici, un échec du test pourrait venir des actions
    // et pas du moteur).
    Actions {
        players: (0..n_players)
            .map(|i| Action {
                move_dir: (0.5, if i % 2 == 0 { 0.3 } else { -0.3 }),
                kick: None,
            })
            .collect(),
    }
}

#[test]
fn same_seed_and_actions_produce_same_final_state() {
    let seed = 1234;
    let config = EngineConfig::default();
    let n = config.players_per_team;

    let mut engine_a = Engine::new(config.clone(), seed);
    let mut engine_b = Engine::new(config, seed);

    engine_a.reset();
    engine_b.reset();

    let mut last_a = None;
    let mut last_b = None;

    for _ in 0..200 {
        let actions = [fixed_actions(n), fixed_actions(n)];
        last_a = Some(engine_a.step(actions.clone()));
        last_b = Some(engine_b.step(actions));
    }

    // On compare via leur représentation `{:?}` plutôt que `==` directement :
    // ça évite de devoir dériver `PartialEq` sur toute la chaîne de types
    // (`StepResult` -> `Observation` -> `Vec<PlayerObs>`...) juste pour ce
    // test, et deux `f32` obtenus par la même suite d'opérations sur la même
    // machine sont bit-à-bit identiques (le déterminisme de rapier2d ne
    // repose pas sur de l'aléatoire caché, seulement sur nos actions et
    // notre `rng` seedé).
    assert_eq!(
        format!("{:?}", last_a.unwrap()),
        format!("{:?}", last_b.unwrap())
    );
}
