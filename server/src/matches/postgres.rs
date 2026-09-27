//! Implémentation `MatchStore` adossée à PostgreSQL — la MÊME base que
//! `frondori-web` (Laravel/Eloquent), qui lira cette table pour afficher la
//! liste des matchs en direct et servir les replays.

use async_trait::async_trait;
use sqlx::PgPool;

use engine::types::SpectatorFrame;

use super::{MatchStore, ENVIRONMENT};
use crate::match_runner::MatchId;

/// `IF NOT EXISTS` : sans danger à ré-exécuter à chaque démarrage, même
/// raisonnement que `auth::postgres::ensure_schema` (pas de système de
/// migration séparé pour l'instant).
const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS matches (
        id             UUID PRIMARY KEY,
        environment    TEXT NOT NULL,
        home_player_id TEXT NOT NULL,
        away_player_id TEXT NOT NULL,
        status         TEXT NOT NULL,
        home_score     INT,
        away_score     INT,
        home_outcome   TEXT,
        replay         JSONB,
        started_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
        ended_at       TIMESTAMPTZ
    )
";

/// Crée la table si elle n'existe pas encore.
pub async fn ensure_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(SCHEMA).execute(pool).await?;
    Ok(())
}

pub struct PostgresMatchStore {
    pool: PgPool,
}

impl PostgresMatchStore {
    /// Réutilise un pool déjà ouvert (partagé avec `PostgresAuthProvider`,
    /// cf. `main.rs`) plutôt que d'en ouvrir un second : les deux
    /// fonctionnalités tapent dans la même base, pas besoin de doubler le
    /// budget de connexions pour ça.
    pub async fn new(pool: PgPool) -> Result<Self, sqlx::Error> {
        ensure_schema(&pool).await?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl MatchStore for PostgresMatchStore {
    async fn record_start(&self, match_id: MatchId, home_player_id: &str, away_player_id: &str) {
        // `$1::uuid` : `match_id` est envoyé comme texte (`.to_string()`) et
        // casté côté SQL, plutôt que de dépendre de la feature `uuid` de
        // `sqlx` (cf. le commentaire sur la dépendance dans `Cargo.toml`).
        let result = sqlx::query(
            "INSERT INTO matches (id, environment, home_player_id, away_player_id, status)
             VALUES ($1::uuid, $2, $3, $4, 'live')",
        )
        .bind(match_id.to_string())
        .bind(ENVIRONMENT)
        .bind(home_player_id)
        .bind(away_player_id)
        .execute(&self.pool)
        .await;

        if let Err(err) = result {
            tracing::error!(%match_id, %err, "échec de l'enregistrement du début de match");
        }
    }

    async fn record_end(
        &self,
        match_id: MatchId,
        home_score: u32,
        away_score: u32,
        home_outcome: protocol::MatchOutcome,
        replay: &[SpectatorFrame],
    ) {
        let replay_json = match serde_json::to_string(replay) {
            Ok(json) => json,
            Err(err) => {
                tracing::error!(%match_id, %err, "échec de sérialisation du replay, match non enregistré");
                return;
            }
        };

        // Même principe que pour l'UUID : `replay_json` est du texte,
        // casté en `jsonb` côté SQL (`$5::jsonb`).
        let result = sqlx::query(
            "UPDATE matches
             SET status = 'finished',
                 home_score = $2,
                 away_score = $3,
                 home_outcome = $4,
                 replay = $5::jsonb,
                 ended_at = now()
             WHERE id = $1::uuid",
        )
        .bind(match_id.to_string())
        .bind(home_score as i32)
        .bind(away_score as i32)
        .bind(outcome_to_str(home_outcome))
        .bind(replay_json)
        .execute(&self.pool)
        .await;

        if let Err(err) = result {
            tracing::error!(%match_id, %err, "échec de l'enregistrement de la fin de match");
        }
    }
}

/// Représentation textuelle explicite (plutôt que de dépendre de
/// `#[derive(Debug)]`, un détail d'implémentation qui pourrait changer sans
/// rapport avec le contrat de cette colonne).
fn outcome_to_str(outcome: protocol::MatchOutcome) -> &'static str {
    match outcome {
        protocol::MatchOutcome::Win => "Win",
        protocol::MatchOutcome::Loss => "Loss",
        protocol::MatchOutcome::Draw => "Draw",
        protocol::MatchOutcome::OpponentForfeit => "OpponentForfeit",
    }
}
