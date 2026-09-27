//! Script de test JETABLE (pas une fonctionnalité du produit) : joue un
//! match complet en local (aucun réseau, comme `random_agent`) entre deux
//! agents id donnés en argument, et persiste le résultat + le replay dans la
//! VRAIE base Postgres partagée avec `frondori-web`, via le même
//! `PostgresMatchStore` que le serveur en production.
//!
//! Sert à valider que `frondori-web` calcule de vraies stats (ELO, possession)
//! à partir d'un replay réel, sans attendre 5 minutes qu'un vrai match réseau
//! se termine (`EngineConfig::default().max_ticks` = 9000 à 30 Hz) : ici, le
//! moteur tourne en boucle serrée, sans throttling temps réel (contrairement
//! à `match_runner`, qui attend un vrai tick d'horloge à chaque itération).
//!
//! Lancer avec :
//! DATABASE_URL="postgres://melvine@localhost/frondori" \
//!   cargo run -p server --example seed_test_match -- <home_agent_id> <away_agent_id>

use engine::config::EngineConfig;
use engine::types::{Action, Actions};
use engine::Engine;
use server::match_runner::MatchId;
use server::matches::{MatchStore, PostgresMatchStore};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let home_player_id = args.get(1).expect("usage: seed_test_match <home_agent_id> <away_agent_id>");
    let away_player_id = args.get(2).expect("usage: seed_test_match <home_agent_id> <away_agent_id>");

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL requise");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connexion Postgres échouée");
    let match_store = PostgresMatchStore::new(pool)
        .await
        .expect("échec d'initialisation du schéma");

    let match_id: MatchId = uuid::Uuid::new_v4();
    match_store.record_start(match_id, home_player_id, away_player_id).await;

    let config = EngineConfig::default();
    let players_per_team = config.players_per_team;
    let mut engine = Engine::new(config, /* seed */ 7);
    engine.reset();

    // Équipe 0 (home) fonce constamment vers +x (donc vers le but adverse,
    // cf. doc `SpectatorPlayer`) et tire dès que possible ; équipe 1 (away)
    // NOOP tout le match. Asymétrie volontaire pour obtenir un résultat et
    // une répartition de possession clairement non triviaux à vérifier.
    let home_action = Action {
        move_dir: (1.0, 0.0),
        kick: Some((1.0, 0.0)),
    };
    let away_action = Action::NOOP;

    let mut replay_frames = vec![engine.spectator_frame()];
    let mut last_score = (0, 0);

    loop {
        let actions = [
            Actions { players: vec![home_action; players_per_team] },
            Actions { players: vec![away_action; players_per_team] },
        ];

        let result = engine.step(actions);
        replay_frames.push(engine.spectator_frame());
        last_score = result.observations[0].score;

        if result.done {
            break;
        }
    }

    let (home_score, away_score) = last_score;
    let outcome = match home_score.cmp(&away_score) {
        std::cmp::Ordering::Greater => protocol::MatchOutcome::Win,
        std::cmp::Ordering::Less => protocol::MatchOutcome::Loss,
        std::cmp::Ordering::Equal => protocol::MatchOutcome::Draw,
    };

    match_store
        .record_end(match_id, home_score, away_score, outcome, &replay_frames)
        .await;

    println!(
        "match {match_id} enregistré : {home_score}-{away_score} ({} frames)",
        replay_frames.len()
    );
}
