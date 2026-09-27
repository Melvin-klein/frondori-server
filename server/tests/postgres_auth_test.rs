//! Test optionnel : vérifie `PostgresAuthProvider` contre une VRAIE base
//! PostgreSQL locale. S'auto-ignore (au lieu d'échouer) si la variable
//! d'environnement `DATABASE_URL` n'est pas définie, pour ne pas casser
//! `cargo test` sur une machine sans PostgreSQL installé/configuré.
//!
//! Lancer avec, par exemple :
//!   DATABASE_URL=postgres://melvine@localhost/frondori cargo test -p server --test postgres_auth_test

use server::auth::{AuthProvider, PostgresAuthProvider};

const TEST_TOKEN: &str = "test-token-integration-postgres-auth";

#[tokio::test]
async fn authenticate_reads_and_rejects_tokens_from_postgres() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL non définie : test ignoré (pas de PostgreSQL local disponible)");
        return;
    };

    let provider = PostgresAuthProvider::connect(&database_url)
        .await
        .expect("connexion à PostgreSQL échouée (DATABASE_URL valide ?)");
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("connexion à PostgreSQL échouée (deuxième pool, pour le nettoyage)");

    // Nettoyage AVANT (au cas où une exécution précédente aurait échoué
    // avant d'atteindre le nettoyage final) ET après, pour que ce test soit
    // rejouable indéfiniment sans polluer la base avec un token de test.
    let cleanup = async {
        sqlx::query("DELETE FROM players WHERE token = $1")
            .bind(TEST_TOKEN)
            .execute(&pool)
            .await
            .expect("échec du nettoyage du token de test");
    };
    cleanup.await;

    let before = provider.authenticate(TEST_TOKEN).await;
    assert!(
        before.is_err(),
        "un token jamais enregistré ne devrait jamais authentifier"
    );

    sqlx::query("INSERT INTO players (token, player_id, display_name) VALUES ($1, $2, $3)")
        .bind(TEST_TOKEN)
        .bind("test-player")
        .bind("Test Intégration")
        .execute(&pool)
        .await
        .expect("échec de l'insertion du token de test");

    let player_id = provider
        .authenticate(TEST_TOKEN)
        .await
        .expect("le token vient d'être enregistré, l'authentification doit réussir");
    assert_eq!(player_id, "test-player");

    sqlx::query("DELETE FROM players WHERE token = $1")
        .bind(TEST_TOKEN)
        .execute(&pool)
        .await
        .expect("échec du nettoyage final du token de test");
}
