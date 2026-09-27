//! Exemple : fait tourner un match complet entre deux équipes jouant des
//! actions aléatoires, entièrement en local (aucun réseau). Sert à la fois
//! de test manuel du moteur et de démonstration de l'API façon Gym.
//!
//! Lancer avec : `cargo run -p engine --example random_agent`
//!
//! (`-p engine` cible le package "engine" du workspace ; sans ça, cargo ne
//! saurait pas dans quel crate chercher l'example si plusieurs en définissent
//! un du même nom.)

use engine::config::EngineConfig;
use engine::types::{Action, Actions};
use engine::Engine;
use rand::Rng;

/// Génère des actions aléatoires pour `n_players` joueurs d'une équipe.
/// `rng: &mut impl Rng` = "un type quelconque qui implémente le trait Rng,
/// passé par référence mutable" : équivalent Rust d'un paramètre générique
/// avec contrainte, similaire à un `Rng*` en C++ mais vérifié à la compilation.
fn random_actions(n_players: usize, rng: &mut impl Rng) -> Actions {
    let players = (0..n_players)
        .map(|_| Action {
            move_dir: (rng.gen_range(-1.0..=1.0), rng.gen_range(-1.0..=1.0)),
            kick: None,
        })
        .collect();
    Actions { players }
}

fn main() {
    // Seed fixe : relancer ce programme produit exactement le même match.
    let seed = 42;
    let config = EngineConfig::default();
    let players_per_team = config.players_per_team;

    let mut engine = Engine::new(config, seed);
    let mut rng = rand::thread_rng(); // non-déterministe : ok ici, seul le *contenu*
                                       // des actions varie, pas la simulation elle-même

    let _initial_observations = engine.reset();

    for tick in 0..300 {
        let actions = [
            random_actions(players_per_team, &mut rng),
            random_actions(players_per_team, &mut rng),
        ];

        let result = engine.step(actions);
        println!(
            "tick {tick}: score A={:?} reward A={:?}",
            result.observations[0].score,
            result.rewards[0].total()
        );

        if result.done {
            println!("Match terminé après {tick} ticks : {:?}", result.info);
            break;
        }
    }
}
