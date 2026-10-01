//! Outil de DEV (pas une fonctionnalité du produit) : le vrai serveur,
//! identique à la production, mais avec une cadence imposée pour jouer un
//! match complet en quelques secondes au lieu de plusieurs minutes (le
//! football dure 5 minutes à sa cadence réelle).
//!
//! - `FRONDORI_TICK_RATE` : pas par seconde imposés (défaut : 300).
//! - `DATABASE_URL` : si définie, tokens et matchs dans Postgres (les matchs
//!   apparaissent alors sur le site). Sinon, deux tokens en mémoire,
//!   `token-a` et `token-b` (valables pour tous les environnements), et
//!   aucune persistance.
//! - `FRONDORI_ENV_WORKER` : commande du worker (cf. `environments.rs`).
//!
//! Écoute sur 127.0.0.1:8081. Lancer avec :
//!   FRONDORI_ENV_WORKER="engine-python/.venv/bin/python -m frondori_engine.worker" \
//!     cargo run -p server --example fast_server

use std::collections::HashMap;
use std::sync::Arc;

use server::auth::{AuthProvider, InMemoryAuthProvider, PostgresAuthProvider};
use server::environments::{describe_environments, WorkerCommand};
use server::match_runner::MatchRunnerConfig;
use server::matches::{MatchStore, NullMatchStore, PostgresMatchStore};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let tick_rate: f64 = std::env::var("FRONDORI_TICK_RATE")
        .ok()
        .and_then(|rate| rate.parse().ok())
        .unwrap_or(300.0);

    let worker_command = WorkerCommand::from_env();
    let catalog = describe_environments(&worker_command)
        .await
        .unwrap_or_else(|err| panic!("impossible de décrire les environnements : {err}"));

    let (auth, match_store): (Arc<dyn AuthProvider>, Arc<dyn MatchStore>) = match std::env::var("DATABASE_URL") {
        Ok(url) => {
            let pool = sqlx::PgPool::connect(&url).await.expect("connexion Postgres échouée");
            (
                Arc::new(PostgresAuthProvider::new(pool.clone()).await.unwrap()),
                Arc::new(PostgresMatchStore::new(pool).await.unwrap()),
            )
        }
        Err(_) => {
            let tokens = HashMap::from([
                ("token-a".to_string(), "agent-a".to_string()),
                ("token-b".to_string(), "agent-b".to_string()),
            ]);
            (Arc::new(InMemoryAuthProvider::new(tokens)), Arc::new(NullMatchStore))
        }
    };

    let config = MatchRunnerConfig {
        tick_rate_override: Some(tick_rate),
        ..MatchRunnerConfig::default()
    };
    match_store.record_environments(&catalog).await;
    let state = server::new_app_state(auth, match_store, catalog, worker_command, config);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8081")
        .await
        .expect("impossible de binder le port 8081");
    println!("serveur accéléré ({tick_rate} pas/s) à l'écoute sur 127.0.0.1:8081");
    axum::serve(listener, server::router(state)).await.unwrap();
}
