//! Script de test JETABLE (pas une fonctionnalité du produit) : un serveur
//! réel (gateway + match runner + réseau, exactement le même code qu'en
//! production), mais avec un match très court (`max_ticks` réduit) et une
//! auth en mémoire (deux tokens fixes) — pour valider rapidement des clients
//! SDK réels contre un vrai serveur, sans attendre les 5 minutes d'un match
//! `EngineConfig::default()` ni toucher à la persistance Postgres.
//!
//! Lancer avec : cargo run -p server --example short_test_server

use std::collections::HashMap;
use std::sync::Arc;

use engine::config::EngineConfig;
use server::auth::InMemoryAuthProvider;
use server::match_runner::MatchRunnerConfig;
use server::matches::NullMatchStore;

#[tokio::main]
async fn main() {
    let mut tokens = HashMap::new();
    tokens.insert("test-fix-home".to_string(), "fix-home".to_string());
    tokens.insert("test-fix-away".to_string(), "fix-away".to_string());
    let auth = Arc::new(InMemoryAuthProvider::new(tokens));

    let engine_config = EngineConfig {
        max_ticks: 60, // 2 secondes à 30 Hz : juste de quoi valider la fin de match normale
        ..EngineConfig::default()
    };

    let state = server::new_app_state(
        auth,
        Arc::new(NullMatchStore),
        engine_config,
        MatchRunnerConfig::default(),
    );
    let app = server::router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8081")
        .await
        .expect("impossible de binder le port 8081");
    println!("serveur de test à l'écoute sur 127.0.0.1:8081");

    axum::serve(listener, app).await.unwrap();
}
