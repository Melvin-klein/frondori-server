//! Implémentation `MatchStore` adossée à PostgreSQL — la MÊME base que
//! `frondori-web` (Laravel/Eloquent), qui lit ces tables pour afficher les
//! matchs, servir les replays et calculer les classements.

use std::collections::HashMap;

use async_trait::async_trait;
use protocol::Value;
use sqlx::PgPool;

use super::{MatchStatus, MatchStore, Participant, ParticipantResult};
use crate::environments::Catalog;
use crate::match_runner::MatchId;

/// `IF NOT EXISTS` : sans danger à ré-exécuter à chaque démarrage (pas de
/// système de migration séparé pour l'instant).
///
/// Un match n'a plus de "home"/"away" : un environnement peut avoir un, deux
/// ou N agents, d'où une table de participants à part (une ligne par
/// agent). `seat` = position de l'agent dans l'ordre de l'environnement.
///
/// `environments` est le catalogue publié à chaque démarrage (cf.
/// `record_environments`). Un environnement retiré du code n'est pas
/// supprimé (ses matchs passés y font référence) : il passe `available =
/// false`.
///
/// Colonnes ajoutées après coup : par `ALTER TABLE ... ADD COLUMN IF NOT
/// EXISTS`, sans danger lui aussi à ré-exécuter, et qui n'efface rien d'une
/// base existante.
const SCHEMA: [&str; 9] = [
    "CREATE TABLE IF NOT EXISTS environments (
        id                 TEXT PRIMARY KEY,
        title              TEXT NOT NULL,
        description        TEXT NOT NULL,
        ranking            TEXT NOT NULL,
        agents             JSONB NOT NULL,
        tick_rate          DOUBLE PRECISION NOT NULL,
        observation_spaces JSONB NOT NULL,
        action_spaces      JSONB NOT NULL,
        available          BOOLEAN NOT NULL DEFAULT true,
        updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
    )",
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
    // Temps de calcul accordé par action (matchs en pas-à-pas).
    "ALTER TABLE environments ADD COLUMN IF NOT EXISTS compute_budget_ms DOUBLE PRECISION",
    // Règles détaillées de l'environnement (Markdown), pour la documentation.
    "ALTER TABLE environments ADD COLUMN IF NOT EXISTS documentation TEXT",
    // Paquet Python à installer pour jouer à l'environnement en local.
    "ALTER TABLE environments ADD COLUMN IF NOT EXISTS package TEXT",
    // Bilan des temps de réponse de chaque participant, et le détail pas par
    // pas (cf. `match_runner::timing`).
    "ALTER TABLE match_participants ADD COLUMN IF NOT EXISTS timing JSONB",
    "ALTER TABLE match_participants ADD COLUMN IF NOT EXISTS step_timings JSONB",
];

/// Clé arbitraire (mais fixe) du verrou qui sérialise la création du schéma.
const SCHEMA_LOCK: i64 = 0x6672_6f6e_646f_7269; // "frondori" en ASCII

pub async fn ensure_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    // `CREATE TABLE IF NOT EXISTS` n'est PAS sûr en concurrence : deux
    // connexions qui créent la même table au même instant voient toutes deux
    // qu'elle n'existe pas, et la seconde échoue (constaté : deux tests
    // lancés en parallèle, "duplicate key ... pg_type_typname_nsp_index").
    // Un verrou consultatif (`pg_advisory_xact_lock`), tenu jusqu'à la fin
    // de la transaction, fait passer les créations l'une après l'autre —
    // entre tests comme entre deux serveurs démarrés en même temps.
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SCHEMA_LOCK)
        .execute(&mut *tx)
        .await?;
    for statement in SCHEMA {
        sqlx::query(statement).execute(&mut *tx).await?;
    }
    tx.commit().await
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

    async fn upsert_environments(&self, catalog: &Catalog) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        for (id, info) in catalog {
            sqlx::query(
                "INSERT INTO environments
                     (id, title, description, documentation, package, ranking, agents, tick_rate,
                      compute_budget_ms, observation_spaces, action_spaces, available, updated_at)
                 VALUES ($1, $2, $3, $4, NULLIF($5, ''), $6, $7::jsonb, $8, $9, $10::jsonb, $11::jsonb, true, now())
                 ON CONFLICT (id) DO UPDATE SET
                     title = EXCLUDED.title,
                     description = EXCLUDED.description,
                     documentation = EXCLUDED.documentation,
                     package = EXCLUDED.package,
                     ranking = EXCLUDED.ranking,
                     agents = EXCLUDED.agents,
                     tick_rate = EXCLUDED.tick_rate,
                     compute_budget_ms = EXCLUDED.compute_budget_ms,
                     observation_spaces = EXCLUDED.observation_spaces,
                     action_spaces = EXCLUDED.action_spaces,
                     available = true,
                     updated_at = now()",
            )
            .bind(id)
            .bind(&info.title)
            .bind(&info.description)
            .bind(&info.documentation)
            .bind(&info.package)
            .bind(&info.ranking)
            .bind(json(&info.agents))
            .bind(info.tick_rate)
            .bind(info.compute_budget_ms)
            .bind(json(&spaces_for_json(&info.observation_spaces)))
            .bind(json(&spaces_for_json(&info.action_spaces)))
            .execute(&mut *tx)
            .await?;
        }

        // `<> ALL($1)` : "différent de chacun des éléments du tableau". Un
        // `Vec<String>` est envoyé tel quel comme `text[]` Postgres.
        let ids: Vec<String> = catalog.keys().cloned().collect();
        sqlx::query("UPDATE environments SET available = false, updated_at = now() WHERE id <> ALL($1)")
            .bind(ids)
            .execute(&mut *tx)
            .await?;

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
                 SET final_return = $3, forfeited = $4, final_info = $5::jsonb,
                     timing = $6::jsonb, step_timings = $7::jsonb
                 WHERE match_id = $1::uuid AND seat = $2",
            )
            .bind(match_id.to_string())
            .bind(result.seat as i32)
            .bind(result.final_return)
            .bind(result.forfeited)
            .bind(result.final_info.to_string())
            .bind(result.timing.to_string())
            .bind(result.step_timings.to_string())
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await
    }
}

#[async_trait]
impl MatchStore for PostgresMatchStore {
    async fn record_environments(&self, catalog: &Catalog) {
        if let Err(err) = self.upsert_environments(catalog).await {
            tracing::error!(%err, "échec de la publication du catalogue des environnements");
        }
    }

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

/// Sérialise en texte JSON, envoyé puis casté en `jsonb` côté SQL (même
/// principe que le replay). Les spaces arrivent du worker en MessagePack
/// (`rmpv::Value`) et ne contiennent que des nombres, chaînes, listes et
/// maps : la conversion ne peut pas échouer en pratique, d'où `expect`.
fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).expect("valeur non convertible en JSON")
}

/// Les spaces, prêts pour JSON. Les bornes d'un `Box` peuvent être infinies
/// (`-inf`/`+inf` : "non borné"), ce que MessagePack représente sans peine
/// mais que JSON ne sait pas écrire : `serde_json` les remplacerait
/// silencieusement par `null` (constaté sur le football), ambigu pour qui lit
/// le catalogue ou une archive de match. On les écrit donc en chaînes
/// `"inf"`/`"-inf"`, que Python relit directement (`float("-inf")`, et numpy
/// avec `np.array(bornes, dtype=float)`).
///
/// Les agents, eux, ne sont pas concernés : ils reçoivent les spaces en
/// MessagePack (`MatchStart`), où l'infini est conservé tel quel.
fn spaces_for_json(spaces: &HashMap<String, Value>) -> HashMap<String, Value> {
    spaces.iter().map(|(agent, space)| (agent.clone(), finite_or_label(space))).collect()
}

/// Copie récursive de `value` où chaque flottant non fini devient une chaîne.
fn finite_or_label(value: &Value) -> Value {
    let label = |x: f64| {
        Value::from(if x.is_nan() {
            "nan"
        } else if x > 0.0 {
            "inf"
        } else {
            "-inf"
        })
    };
    match value {
        Value::F32(x) if !x.is_finite() => label(f64::from(*x)),
        Value::F64(x) if !x.is_finite() => label(*x),
        Value::Array(items) => Value::Array(items.iter().map(finite_or_label).collect()),
        Value::Map(entries) => Value::Map(
            entries
                .iter()
                .map(|(key, item)| (key.clone(), finite_or_label(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}
