//! Bibliothèque du serveur : c'est ici que vivent les modules (`auth`,
//! `gateway`, `match_runner`). `main.rs` n'est qu'un point d'entrée fin qui
//! assemble ces briques et démarre l'écoute réseau.
//!
//! Pourquoi séparer un `lib.rs` d'un `main.rs` alors qu'il n'y a qu'un seul
//! binaire au final ? Parce qu'un binaire (`[[bin]]`) ne peut pas être
//! importé par un autre crate — même pas par les tests d'intégration
//! (`tests/`) du même package. En exposant la logique via une bibliothèque,
//! `server/tests/smoke_test.rs` peut faire `use server::gateway::...` et
//! démarrer un vrai serveur en mémoire pour un test de bout en bout, sans
//! dupliquer la moindre ligne de logique.

pub mod auth;
pub mod gateway;
pub mod match_runner;
pub mod matches;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::routing::get;
use axum::Router;
use engine::EngineConfig;

use crate::auth::AuthProvider;
use crate::gateway::state::AppState;
use crate::match_runner::MatchRunnerConfig;
use crate::matches::MatchStore;

/// Construit l'état partagé initial : queue de matchmaking vide, registre de
/// spectateurs vide, plus les deux configurations transmises telles quelles
/// à chaque nouveau match.
pub fn new_app_state(
    auth: Arc<dyn AuthProvider>,
    match_store: Arc<dyn MatchStore>,
    engine_config: EngineConfig,
    match_config: MatchRunnerConfig,
) -> AppState {
    AppState {
        auth,
        match_store,
        matchmaking_queue: Arc::new(Mutex::new(VecDeque::new())),
        spectators: Arc::new(Mutex::new(HashMap::new())),
        engine_config,
        match_config,
    }
}

/// Construit le routeur axum complet : `/agent` (joueurs) et
/// `/spectate/:match_id` (spectateurs, lecture seule). Séparé de `main` pour
/// être réutilisable tel quel par les tests d'intégration, qui ont besoin
/// d'un serveur réel (écoutant sur un vrai port TCP) pour parler WebSocket
/// avec de vrais clients.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/agent", get(gateway::agent_ws_handler))
        .route("/spectate/:match_id", get(gateway::spectate_ws_handler))
        .with_state(state)
}
