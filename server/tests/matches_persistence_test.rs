//! Test optionnel : vérifie que `PostgresMatchStore` enregistre bien un
//! match de bout en bout (participants, retours, replay) dans une VRAIE
//! base PostgreSQL locale. S'auto-ignore si `DATABASE_URL` n'est pas
//! définie, même principe que `postgres_auth_test.rs`.
//!
//! Lancer avec, par exemple :
//!   DATABASE_URL=postgres://melvine@localhost/frondori cargo test -p server --test matches_persistence_test

mod common;

use std::sync::Arc;

use server::matches::PostgresMatchStore;

use common::{play_match, Behaviour, Outcome};

#[tokio::test]
async fn finished_match_is_persisted_with_participants_and_replay() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL non définie : test ignoré (pas de PostgreSQL local disponible)");
        return;
    };
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("connexion à PostgreSQL échouée");
    let store = PostgresMatchStore::new(pool.clone())
        .await
        .expect("initialisation du schéma des matchs échouée");

    let addr = common::start_test_server_with(Arc::new(store), 1000.0).await;
    let url = format!("ws://{addr}/agent");
    let (match_id_tx, match_id_rx) = tokio::sync::oneshot::channel();
    let first = Behaviour {
        match_id_tx: Some(match_id_tx),
        ..Behaviour::default()
    };
    let (a, _b) = tokio::join!(
        play_match(url.clone(), "token-a", "kitchen-v0", first),
        play_match(url, "token-b", "kitchen-v0", Behaviour::default()),
    );
    assert!(matches!(a, Outcome::Finished(_)));
    let match_id = match_id_rx.await.expect("le match_id n'a jamais été reçu");

    // `record_end` s'exécute juste APRÈS l'envoi du `MatchEnd` aux clients :
    // on attend qu'il ait eu lieu plutôt que de lire trop tôt.
    let mut status = String::new();
    for _ in 0..50 {
        status = sqlx::query_scalar("SELECT status FROM matches WHERE id = $1::uuid")
            .bind(&match_id)
            .fetch_one(&pool)
            .await
            .expect("le match n'a pas été trouvé en base");
        if status != "live" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // `::text` : colonnes JSONB lues en texte et parsées ici, sans dépendre
    // de la feature `json` de `sqlx` (cf. `Cargo.toml`).
    let (environment, replay): (String, String) =
        sqlx::query_as("SELECT environment, replay::text FROM matches WHERE id = $1::uuid")
            .bind(&match_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut participants: Vec<(i32, String, String, Option<f64>, bool, String)> = sqlx::query_as(
        "SELECT seat, agent, player_id, final_return, forfeited, final_info::text
         FROM match_participants WHERE match_id = $1::uuid ORDER BY seat",
    )
    .bind(&match_id)
    .fetch_all(&pool)
    .await
    .unwrap();

    // Nettoyage juste après la lecture, AVANT les assertions : une assertion
    // ratée ne laisse ainsi aucun match de test en base. Les participants
    // suivent via `ON DELETE CASCADE`.
    sqlx::query("DELETE FROM matches WHERE id = $1::uuid")
        .bind(&match_id)
        .execute(&pool)
        .await
        .expect("échec du nettoyage du match de test");

    assert_eq!(status, "finished");
    assert_eq!(environment, "kitchen-v0");

    // Qui joue quel chef dépend de l'ordre d'arrivée : on vérifie le lien
    // agent <-> siège, et l'ensemble des joueurs, pas une attribution fixe.
    assert_eq!(participants.len(), 2);
    participants.sort_by_key(|p| p.0);
    assert_eq!((participants[0].0, participants[0].1.as_str()), (0, "chef_0"));
    assert_eq!((participants[1].0, participants[1].1.as_str()), (1, "chef_1"));
    let mut players: Vec<&str> = participants.iter().map(|p| p.2.as_str()).collect();
    players.sort();
    assert_eq!(players, ["agent-a", "agent-b"]);
    for (_, _, _, final_return, forfeited, final_info) in &participants {
        assert_eq!(*final_return, Some(0.0));
        assert!(!forfeited);
        // Infos propres à l'environnement, gardées telles quelles.
        let info: serde_json::Value = serde_json::from_str(final_info).unwrap();
        assert_eq!(info["served"], 0);
    }

    // Une scène initiale + une par pas (200), chacune au format générique.
    let scenes: serde_json::Value = serde_json::from_str(&replay).expect("replay JSON invalide");
    let scenes = scenes.as_array().expect("le replay devrait être un tableau");
    assert_eq!(scenes.len(), 201);
    assert!(scenes[0]["shapes"].is_array());
}
