//! Implémentation `MatchStore` adossée à PostgreSQL — la MÊME base que
//! `frondori-web` (Laravel/Eloquent), qui lit ces tables pour afficher les
//! matchs, servir les replays et calculer les classements.

use async_trait::async_trait;
use sqlx::PgPool;

use super::{MatchStatus, MatchStore, Participant, ParticipantResult};
use crate::match_runner::MatchId;

/// `IF NOT EXISTS` : sans danger à ré-exécuter à chaque démarrage (pas de
/// système de migration séparé pour l'instant).
///
/// Un match n'a plus de "home"/"away" : un environnement peut avoir un, deux
/// ou N agents, d'où une table de participants à part (une ligne par
/// agent). `seat` = position de l'agent dans l'ordre de l'environnement.
const SCHEMA: [&str; 3] = [
    "CREATE TABLE IF NOT EXISTS matches (
        id          UUID PRIMARY KEY,
        environment TEXT NOT NULL,
        status      TEXT NOT NULL,
        replay      JSONB,
        started_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
        ended_at    TIMESTAMPTZ
    )",
    "CREATE TABLE IF NOT EXISTS match_participants (
        match_id     UUID NOT NULL REFERENCES matches (id) ON DELETE CASCADE,
        seat         INT NOT NULL,
        agent        TEXT NOT NULL,
        player_id    TEXT NOT NULL,
        final_return DOUBLE PRECISION,
        forfeited    BOOLEAN NOT NULL DEFAULT false,
        final_info   JSONB,
        PRIMARY KEY (match_id, seat)
    )",
    "CREATE INDEX IF NOT EXISTS match_participants_player_id_idx ON match_participants (player_id)",
];

pub async fn ensure_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    for statement in SCHEMA {
        sqlx::query(statement).execute(pool).await?;
    }
    Ok(())
}

pub struct PostgresMatchStore {
    pool: PgPool,
}

impl PostgresMatchStore {
    /// Réutilise un pool déjà ouvert (partagé avec `PostgresAuthProvider`,
    /// cf. `main.rs`) plutôt que d'en ouvrir un second.
    pub async fn new(pool: PgPool) -> Result<Self, sqlx::Error> {
        ensure_schema(&pool).await?;
        Ok(Self { pool })
    }

    async fn insert_start(
        &self,
        match_id: MatchId,
        environment: &str,
        participants: &[Participant],
    ) -> Result<(), sqlx::Error> {
        // Une transaction : un match n'existe jamais sans ses participants
        // (le site verrait sinon un match "live" dont on ignore qui le joue).
        // Si une requête échoue, `tx` est détruite sans `commit()`, ce qui
        // annule tout ce qui a été fait (rollback automatique au `drop`).
        let mut tx = self.pool.begin().await?;

        // `$1::uuid` : `match_id` est envoyé comme texte et casté côté SQL,
        // plutôt que de dépendre de la feature `uuid` de `sqlx`.
        sqlx::query("INSERT INTO matches (id, environment, status) VALUES ($1::uuid, $2, 'live')")
            .bind(match_id.to_string())
            .bind(environment)
            .execute(&mut *tx)
            .await?;

        for participant in participants {
            sqlx::query(
                "INSERT INTO match_participants (match_id, seat, agent, player_id)
                 VALUES ($1::uuid, $2, $3, $4)",
            )
            .bind(match_id.to_string())
            .bind(participant.seat as i32)
            .bind(&participant.agent)
            .bind(&participant.player_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await
    }

    async fn update_end(
        &self,
        match_id: MatchId,
        status: MatchStatus,
        results: &[ParticipantResult],
        replay_json: &str,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        // Même principe que pour l'UUID : `replay_json` est du texte, casté
        // en `jsonb` côté SQL.
        sqlx::query(
            "UPDATE matches SET status = $2, replay = $3::jsonb, ended_at = now() WHERE id = $1::uuid",
        )
        .bind(match_id.to_string())
        .bind(status.as_str())
        .bind(replay_json)
        .execute(&mut *tx)
        .await?;

        for result in results {
            sqlx::query(
                "UPDATE match_participants
                 SET final_return = $3, forfeited = $4, final_info = $5::jsonb
                 WHERE match_id = $1::uuid AND seat = $2",
            )
            .bind(match_id.to_string())
            .bind(result.seat as i32)
            .bind(result.final_return)
            .bind(result.forfeited)
            .bind(result.final_info.to_string())
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await
    }
}

#[async_trait]
impl MatchStore for PostgresMatchStore {
    async fn record_start(&self, match_id: MatchId, environment: &str, participants: &[Participant]) {
        if let Err(err) = self.insert_start(match_id, environment, participants).await {
            tracing::error!(%match_id, %err, "échec de l'enregistrement du début de match");
        }
    }

    async fn record_end(
        &self,
        match_id: MatchId,
        status: MatchStatus,
        results: &[ParticipantResult],
        replay_json: &str,
    ) {
        if let Err(err) = self.update_end(match_id, status, results, replay_json).await {
            tracing::error!(%match_id, %err, "échec de l'enregistrement de la fin de match");
        }
    }
}
