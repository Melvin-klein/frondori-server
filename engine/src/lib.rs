//! `engine` : moteur de simulation physique du jeu, pur (aucune dépendance
//! réseau, aucune notion de socket ou d'authentification).
//!
//! API façon "OpenAI Gym" : on crée un [`Engine`], on appelle [`Engine::reset`]
//! une fois, puis on boucle sur [`Engine::step`] en lui donnant les actions de
//! chaque équipe. Voir `examples/random_agent.rs` pour un exemple complet
//! d'utilisation en standalone (sans le serveur).
//!
//! Organisation du crate :
//! - `config` : paramètres de simulation (`EngineConfig`).
//! - `types`  : types de données échangés avec l'extérieur (`Action`,
//!   `Observation`, `Reward`...). Ce sont ces types que `protocol` réutilise
//!   tels quels pour les envoyer sur le réseau.
//! - `sim`    : le moteur lui-même (`Engine`), qui encapsule l'état rapier2d.

// En Rust, un fichier `xxx.rs` à côté de `lib.rs` (ou un dossier `xxx/` avec
// un `mod.rs`/`xxx.rs`) devient un module accessible via `mod xxx;`.
// Par défaut un module est privé : il faut `pub mod` pour le rendre visible
// depuis l'extérieur du crate (ici, depuis `protocol` et `server`).
mod sim;

pub mod config;
pub mod types;

// `pub use` réexporte des éléments d'un sous-module à la racine du crate,
// pour que les autres crates puissent écrire `engine::Engine` et
// `engine::EngineConfig` au lieu de `engine::sim::Engine` et
// `engine::config::EngineConfig`. C'est purement une question d'ergonomie
// d'API, très courant dans les crates Rust idiomatiques.
pub use config::EngineConfig;
pub use sim::Engine;
