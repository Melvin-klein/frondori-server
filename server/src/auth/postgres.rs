//! Implémentation `AuthProvider` adossée à une vraie base PostgreSQL, pour
//! que les tokens des participants survivent aux redémarrages du serveur
//! (contrairement à `InMemoryAuthProvider`).

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use super::{AuthError, AuthProvider, PlayerId};

/// `IF NOT EXISTS` : sans danger à ré-exécuter à chaque démarrage. Une seule
/// table pour une seule information (le lien token -> participant) ne
/// justifie pas un système de migrations dédié (`sqlx migrate`, `refinery`...).
///
/// Aucun environnement ici : un token identifie un agent, qui peut jouer à
/// n'importe quel environnement (choisi à chaque connexion, cf. `Hello`).
const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS players (
        token        TEXT PRIMARY KEY,
        player_id    TEXT NOT NULL UNIQUE,
        display_name TEXT NOT NULL,
        created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
    )
";

/// Crée la table si elle n'existe pas encore. Exposée séparément de
/// `PostgresAuthProvider::connect` pour être réutilisée telle quelle par
/// l'outil `manage-tokens` (`src/bin/manage_tokens.rs`), qui parle
/// directement à la base sans passer par le trait `AuthProvider`.
pub async fn ensure_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(SCHEMA).execute(pool).await?;
    Ok(())
}

/// `AuthProvider` adossé à PostgreSQL. Utilise un pool de connexions (pas
/// une connexion par requête) : `sqlx` gère l'attente/le recyclage des
/// connexions en interne, `authenticate` n'a qu'à emprunter le pool.
pub struct PostgresAuthProvider {
    pool: PgPool,
}

impl PostgresAuthProvider {
    /// Réutilise un pool déjà ouvert (partagé avec `PostgresMatchStore`,
    /// cf. `main.rs`) : les deux fonctionnalités tapent dans la même base,
    /// pas besoin d'ouvrir un deuxième pool de connexions pour ça.
    pub async fn new(pool: PgPool) -> Result<Self, sqlx::Error> {
        ensure_schema(&pool).await?;
        Ok(Self { pool })
    }

    /// Ouvre son propre pool à partir de `database_url`
    /// (ex: `postgres://user@localhost/frondori`), puis délègue à `new`.
    /// Pratique quand on n'a pas déjà un pool sous la main (tests,
    /// `manage-tokens`) ; `main.rs` utilise `new` directement pour partager
    /// le pool avec `PostgresMatchStore`.
    pub async fn connect(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;
        Self::new(pool).await
    }
}

#[async_trait]
impl AuthProvider for PostgresAuthProvider {
    async fn authenticate(&self, token: &str) -> Result<PlayerId, AuthError> {
        // `$1` (pas d'interpolation de `token` dans la chaîne SQL) : la
        // valeur est envoyée séparément de la requête préparée, ce qui
        // élimine par construction toute injection SQL via ce paramètre.
        let player_id: Option<String> =
            sqlx::query_scalar("SELECT player_id FROM players WHERE token = $1")
                .bind(token)
                .fetch_optional(&self.pool)
                .await
                // Le détail de l'erreur reste dans les logs du serveur : le
                // renvoyer au client exposerait l'intérieur de la base.
                .map_err(|err| {
                    tracing::error!(error = %err, "authentification : requête Postgres en échec");
                    AuthError {
                        reason: "server error, please retry later".to_string(),
                    }
                })?;

        // Messages en anglais : ils arrivent tels quels chez les participants
        // (exception `AuthenticationError` du SDK).
        player_id.ok_or_else(|| AuthError {
            reason: "unknown token".to_string(),
        })
    }
}
