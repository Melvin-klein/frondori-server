//! Outil d'administration en ligne de commande pour enregistrer/révoquer les
//! tokens des participants dans PostgreSQL. Un DEUXIÈME binaire dans le même
//! crate (Cargo compile chaque fichier de `src/bin/` comme un exécutable
//! séparé) : il partage `server::auth::postgres`, mais n'a rien à voir avec
//! le serveur lui-même (pas de tokio::main sur un port réseau, pas de
//! gateway) — juste une poignée de requêtes SQL ponctuelles.
//!
//! Usage :
//!   DATABASE_URL=postgres://localhost/frondori cargo run --bin manage-tokens -- add <token> <player_id> <nom>
//!   DATABASE_URL=postgres://localhost/frondori cargo run --bin manage-tokens -- list
//!   DATABASE_URL=postgres://localhost/frondori cargo run --bin manage-tokens -- remove <token>

use clap::{Parser, Subcommand};
use sqlx::postgres::PgPoolOptions;

use server::auth::postgres::ensure_schema;

#[derive(Parser)]
#[command(about = "Gère les tokens des participants enregistrés en base")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Enregistre un participant (ou remplace son token/nom si déjà présent).
    Add {
        token: String,
        player_id: String,
        display_name: String,
    },
    /// Liste tous les participants enregistrés.
    List,
    /// Révoque un token : le participant associé ne pourra plus se connecter.
    Remove { token: String },
}

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    tracing_subscriber::fmt::init();

    let database_url = std::env::var("DATABASE_URL")
        .expect("variable d'environnement DATABASE_URL manquante (ex: postgres://localhost/frondori)");

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;
    ensure_schema(&pool).await?;

    match Cli::parse().command {
        Command::Add {
            token,
            player_id,
            display_name,
        } => {
            // `ON CONFLICT (token) DO UPDATE` : ré-enregistrer un token déjà
            // connu met juste à jour le participant associé (utile pour
            // corriger une faute de frappe sur le nom sans avoir à
            // supprimer/recréer la ligne).
            sqlx::query(
                "INSERT INTO players (token, player_id, display_name)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (token)
                 DO UPDATE SET player_id = excluded.player_id, display_name = excluded.display_name",
            )
            .bind(&token)
            .bind(&player_id)
            .bind(&display_name)
            .execute(&pool)
            .await?;
            println!("Token enregistré pour {display_name} ({player_id}).");
        }
        Command::List => {
            let rows: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT token, player_id, display_name FROM players ORDER BY created_at",
            )
            .fetch_all(&pool)
            .await?;

            if rows.is_empty() {
                println!("Aucun participant enregistré.");
            }
            for (token, player_id, display_name) in rows {
                println!("{token}  {player_id}  {display_name}");
            }
        }
        Command::Remove { token } => {
            let result = sqlx::query("DELETE FROM players WHERE token = $1")
                .bind(&token)
                .execute(&pool)
                .await?;
            if result.rows_affected() == 0 {
                println!("Aucun participant avec ce token.");
            } else {
                println!("Token révoqué.");
            }
        }
    }

    Ok(())
}
