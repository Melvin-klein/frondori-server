//! Point d'entrée du binaire serveur. Toute la logique vit dans `lib.rs` et
//! ses sous-modules (`auth`, `gateway`, `match_runner`, `matches`) : ce
//! fichier ne fait qu'assembler la configuration de production et démarrer
//! l'écoute réseau.

use std::collections::HashMap;
use std::sync::Arc;

use engine::EngineConfig;
use server::auth::{AuthProvider, InMemoryAuthProvider, PostgresAuthProvider};
use server::match_runner::MatchRunnerConfig;
use server::matches::{MatchStore, NullMatchStore, PostgresMatchStore};
use sqlx::postgres::PgPoolOptions;

// `#[tokio::main]` transforme `async fn main()` en `fn main()` classique qui
// démarre un runtime tokio (le "scheduler" qui exécute les tâches async) et
// y lance le corps de la fonction. Sans cette macro, on ne pourrait pas
// utiliser `.await` directement dans `main`.
#[tokio::main]
async fn main() {
    // Initialise `tracing` : permet d'utiliser `tracing::info!`/`debug!`
    // partout dans le code, avec un affichage formaté sur stdout. Contrôlable
    // via la variable d'environnement `RUST_LOG` (ex: `RUST_LOG=debug`).
    tracing_subscriber::fmt::init();

    let (auth, match_store) = build_persistence().await;
    let state = server::new_app_state(
        auth,
        match_store,
        EngineConfig::default(),
        MatchRunnerConfig::default(),
    );
    let app = server::router(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
        .await
        .expect("impossible de binder le port 8080");
    tracing::info!("serveur à l'écoute sur {}", listener.local_addr().unwrap());

    axum::serve(listener, app)
        .await
        .expect("erreur fatale du serveur axum");
}

/// Choisit l'authentification ET la persistance des matchs selon
/// l'environnement, à partir d'un pool Postgres PARTAGÉ entre les deux (pas
/// la peine d'ouvrir deux pools séparés vers la même base) :
/// - `DATABASE_URL` définie => les deux sont adossées à PostgreSQL. Une
///   erreur de connexion ici est fatale : si l'opérateur a explicitement
///   demandé Postgres, servir quand même en mémoire (donc sans AUCUN token
///   valide, ni aucune persistance des matchs) serait silencieusement pire
///   qu'un crash au démarrage.
/// - sinon => authentification en mémoire (table vide, aucun token accepté)
///   et persistance désactivée (`NullMatchStore`, les matchs restent
///   jouables mais rien n'est enregistré). Pratique pour un `cargo run`
///   local rapide ; un avertissement est loggé pour ne pas laisser croire
///   que c'est un mode de production.
async fn build_persistence() -> (Arc<dyn AuthProvider>, Arc<dyn MatchStore>) {
    match std::env::var("DATABASE_URL") {
        Ok(database_url) => {
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(&database_url)
                .await
                .expect("connexion à PostgreSQL échouée (DATABASE_URL invalide ?)");

            let auth = PostgresAuthProvider::new(pool.clone())
                .await
                .expect("échec d'initialisation du schéma d'authentification");
            let match_store = PostgresMatchStore::new(pool)
                .await
                .expect("échec d'initialisation du schéma de persistance des matchs");

            tracing::info!("authentification + persistance des matchs: PostgreSQL");
            (Arc::new(auth), Arc::new(match_store))
        }
        Err(_) => {
            tracing::warn!(
                "DATABASE_URL non définie : authentification en mémoire (aucun token accepté) \
                 et persistance des matchs désactivée. Définir DATABASE_URL pour une utilisation \
                 réelle (cf. `manage-tokens` pour enregistrer des participants)."
            );
            (
                Arc::new(InMemoryAuthProvider::new(HashMap::new())),
                Arc::new(NullMatchStore),
            )
        }
    }
}
