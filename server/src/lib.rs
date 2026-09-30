//! Bibliothèque du serveur : c'est ici que vivent les modules (`auth`,
//! `environments`, `gateway`, `match_runner`, `matches`). `main.rs` n'est
//! qu'un point d'entrée fin qui assemble ces briques et démarre l'écoute.
//!
//! Pourquoi séparer un `lib.rs` d'un `main.rs` alors qu'il n'y a qu'un seul
//! binaire au final ? Parce qu'un binaire ne peut pas être importé par un
//! autre crate — même pas par les tests d'intégration (`tests/`) du même
//! package. En exposant la logique via une bibliothèque, les tests peuvent
//! démarrer un vrai serveur en mémoire sans dupliquer la moindre logique.

pub mod auth;
pub mod environments;
pub mod gateway;
pub mod match_runner;
pub mod matches;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::routing::get;
use axum::Router;

use crate::auth::AuthProvider;
use crate::environments::{Catalog, WorkerCommand};
use crate::gateway::state::AppState;
use crate::match_runner::MatchRunnerConfig;
use crate::matches::MatchStore;

/// Construit l'état partagé initial : files d'attente et registre de
/// spectateurs vides, plus la configuration transmise à chaque match.
pub fn new_app_state(
    auth: Arc<dyn AuthProvider>,
    match_store: Arc<dyn MatchStore>,
    catalog: Catalog,
    worker_command: WorkerCommand,
    match_config: MatchRunnerConfig,
) -> AppState {
    AppState {
        auth,
        match_store,
        catalog: Arc::new(catalog),
        worker_command,
        matchmaking_queues: Arc::new(Mutex::new(HashMap::new())),
        match_config,
        spectators: Arc::new(Mutex::new(HashMap::new())),
    }
}

/// Construit le routeur axum complet : `/agent` (participants) et
/// `/spectate/:match_id` (spectateurs, lecture seule).
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/agent", get(gateway::agent_ws_handler))
        .route("/spectate/:match_id", get(gateway::spectate_ws_handler))
        .with_state(state)
}
