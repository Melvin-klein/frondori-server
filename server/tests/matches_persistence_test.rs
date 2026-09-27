//! Test optionnel : vérifie que `PostgresMatchStore` enregistre bien un
//! match de bout en bout (statut, score, résultat, replay complet) dans une
//! VRAIE base PostgreSQL locale. S'auto-ignore (au lieu d'échouer) si
//! `DATABASE_URL` n'est pas définie, même principe que
//! `postgres_auth_test.rs`.
//!
//! Lancer avec, par exemple :
//!   DATABASE_URL=postgres://melvine@localhost/frondori cargo test -p server --test matches_persistence_test

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use engine::config::EngineConfig;
use server::auth::InMemoryAuthProvider;
use server::match_runner::MatchRunnerConfig;
use server::matches::PostgresMatchStore;

#[tokio::test]
async fn finished_match_is_persisted_with_full_replay() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL non définie : test ignoré (pas de PostgreSQL local disponible)");
        return;
    };

    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("connexion à PostgreSQL échouée");
    let match_store = Arc::new(
        PostgresMatchStore::new(pool.clone())
            .await
            .expect("initialisation du schéma `matches` échouée"),
    );

    let mut tokens = HashMap::new();
    tokens.insert("token-a".to_string(), "player-a".to_string());
    tokens.insert("token-b".to_string(), "player-b".to_string());
    let auth = Arc::new(InMemoryAuthProvider::new(tokens));

    // Match volontairement court : on veut juste vérifier que la
    // persistance fonctionne, pas rejouer un match complet de 5 minutes.
    let engine_config = EngineConfig {
        players_per_team: 1,
        max_ticks: 5,
        ..EngineConfig::default()
    };
    let state = server::new_app_state(
        auth,
        match_store,
        engine_config,
        MatchRunnerConfig::default(),
    );
    let app = server::router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("impossible de binder un port de test");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let agent_url = format!("ws://{addr}/agent");
    let (match_id_tx, match_id_rx) = tokio::sync::oneshot::channel();
    let (end_a, _end_b) = tokio::join!(
        common::play_dummy_match(agent_url.clone(), "token-a", Some(match_id_tx)),
        common::play_dummy_match(agent_url, "token-b", None),
    );
    let match_id = match_id_rx.await.expect("le match_id n'a jamais été reçu");
    assert_eq!(end_a.outcome, protocol::MatchOutcome::Draw);

    // `replay::text` : on lit la colonne JSONB comme du texte brut, qu'on
    // parse nous-mêmes avec `serde_json` — évite de dépendre de la feature
    // `json` de `sqlx` (non activée, cf. le commentaire dans `Cargo.toml`),
    // pour rester cohérent avec l'écriture (`$N::jsonb` côté `postgres.rs`).
    let row: (String, Option<i32>, Option<i32>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT status, home_score, away_score, home_outcome, replay::text
         FROM matches WHERE id = $1::uuid",
    )
    .bind(&match_id)
    .fetch_one(&pool)
    .await
    .expect("le match n'a pas été trouvé en base après sa fin");

    let (status, home_score, away_score, home_outcome, replay_text) = row;

    // Le nettoyage a lieu APRÈS toutes les vérifications (pas avant) : si
    // une assertion échoue, la ligne reste en base pour inspection manuelle.
    let cleanup = async {
        sqlx::query("DELETE FROM matches WHERE id = $1::uuid")
            .bind(&match_id)
            .execute(&pool)
            .await
            .expect("échec du nettoyage de la ligne de test");
    };

    assert_eq!(status, "finished");
    assert_eq!(home_score, Some(0));
    assert_eq!(away_score, Some(0));
    assert_eq!(home_outcome.as_deref(), Some("Draw"));

    let replay_text = replay_text.expect("le replay ne devrait pas être NULL");
    let frames: serde_json::Value =
        serde_json::from_str(&replay_text).expect("le replay n'est pas du JSON valide");
    let frames = frames.as_array().expect("le replay devrait être un tableau");
    // Une frame initiale (tick 0) + une par tick joué (5 ticks) = 6.
    assert_eq!(frames.len(), 6, "nombre de frames de replay inattendu");
    assert!(frames[0].get("players").is_some());

    cleanup.await;
}
