//! Tests de bout en bout : un vrai serveur (vrai port TCP), de vrais workers
//! Python qui exécutent de vrais environnements, et de vrais clients
//! WebSocket (`tokio-tungstenite`, pas `axum::extract::ws` : on joue le rôle
//! d'un SDK participant, pas de la même implémentation que le serveur).

mod common;

use std::time::Duration;

use common::{play_match, Behaviour, Outcome};

fn finished(outcome: Outcome) -> common::MatchReport {
    match outcome {
        Outcome::Finished(report) => report,
        other => panic!("le match aurait dû aller à son terme : {other:?}"),
    }
}

#[tokio::test]
async fn two_chefs_play_a_full_kitchen_match() {
    let addr = common::start_test_server().await;
    let url = format!("ws://{addr}/agent");

    // En parallèle : le matchmaking n'apparie ces deux joueurs qu'au moment
    // où le second arrive pendant que le premier attend déjà en file.
    let (a, b) = tokio::join!(
        play_match(url.clone(), "token-a", "kitchen-v0", Behaviour::default()),
        play_match(url, "token-b", "kitchen-v0", Behaviour::default()),
    );
    let (a, b) = (finished(a), finished(b));

    // Chacun contrôle un chef différent, et connaît son space d'action.
    let mut agents = [a.start.agent.as_str(), b.start.agent.as_str()];
    agents.sort();
    assert_eq!(agents, ["chef_0", "chef_1"]);
    assert_eq!(a.start.environment, "kitchen-v0");
    assert_eq!(a.start.agents, ["chef_0", "chef_1"]);
    assert_eq!(common::field(&a.start.action_space, "n").and_then(|n| n.as_u64()), Some(6));

    // Observation initiale + une par pas (200 pas par défaut), et chaque
    // action envoyée a été acceptée (ou, au pire, est arrivée trop tard).
    assert_eq!(a.observations, 201);
    assert_eq!(a.rejected, 0);
    assert_eq!(a.applied + a.missing, 200);
    // Personne n'a rien cuisiné : 0 pour les deux, aucun forfait.
    assert_eq!(a.end.returns.get("chef_0"), Some(&0.0));
    assert_eq!(a.end.returns.get("chef_1"), Some(&0.0));
    assert!(a.end.forfeited.is_empty());
    assert_eq!(a.end.returns, b.end.returns);
}

#[tokio::test]
async fn a_disconnection_ends_the_match_as_a_forfeit() {
    let addr = common::start_test_server().await;
    let url = format!("ws://{addr}/agent");

    let quitter = Behaviour {
        disconnect_after: Some(10),
        ..Behaviour::default()
    };
    let (quitter, stayer) = tokio::join!(
        play_match(url.clone(), "token-a", "football-v0", quitter),
        play_match(url, "token-b", "football-v0", Behaviour::default()),
    );

    assert!(matches!(quitter, Outcome::Left));
    let stayer = finished(stayer);
    // Le football passe par le même chemin : son space d'action est une Box
    // (N joueurs x 5), que le client a su remplir.
    let shape = common::field(&stayer.start.action_space, "shape").and_then(|s| s.as_array());
    assert_eq!(shape.map(|dims| dims.len()), Some(2));
    // Le forfait est attribué à l'agent du participant parti, pas à l'autre.
    let quitter_agent = if stayer.start.agent == "team_0" { "team_1" } else { "team_0" };
    assert_eq!(stayer.end.forfeited, [quitter_agent]);
}

#[tokio::test]
async fn invalid_actions_are_replaced_without_breaking_the_match() {
    let addr = common::start_test_server().await;
    let url = format!("ws://{addr}/agent");

    let garbage = || Behaviour {
        action: common::garbage_action,
        ..Behaviour::default()
    };
    let (a, b) = tokio::join!(
        play_match(url.clone(), "token-a", "kitchen-v0", garbage()),
        play_match(url, "token-b", "kitchen-v0", garbage()),
    );

    // Chaque action est refusée par le worker et remplacée par l'action
    // neutre — et le client en est informé — mais le match va à son terme,
    // sans forfait.
    for report in [finished(a), finished(b)] {
        assert!(report.end.forfeited.is_empty());
        assert_eq!(report.applied, 0);
        assert!(report.rejected > 0);
        assert_eq!(report.rejected + report.missing, 200);
    }
}

#[tokio::test]
async fn an_unavailable_environment_is_rejected() {
    let addr = common::start_test_server().await;

    let outcome = play_match(format!("ws://{addr}/agent"), "token-a", "chess-v0", Behaviour::default()).await;

    let Outcome::Rejected(reason) = outcome else {
        panic!("la connexion aurait dû être refusée : {outcome:?}");
    };
    assert!(reason.contains("chess-v0"), "raison : {reason}");
}

#[tokio::test]
async fn an_unknown_token_is_rejected() {
    let addr = common::start_test_server().await;

    let outcome = play_match(format!("ws://{addr}/agent"), "token-inconnu", "kitchen-v0", Behaviour::default()).await;

    assert!(matches!(outcome, Outcome::Rejected(_)), "{outcome:?}");
}

#[tokio::test]
async fn one_agent_can_play_several_environments_whose_queues_stay_separate() {
    let addr = common::start_test_server().await;
    let url = format!("ws://{addr}/agent");

    // L'agent A attend seul dans la file du football...
    let football = tokio::spawn(play_match(url.clone(), "token-a", "football-v0", Behaviour::default()));
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ... et, avec le MÊME token, joue en même temps un match de cuisine
    // contre l'agent B, qui ne demandait que la cuisine.
    let (a, b) = tokio::join!(
        play_match(url.clone(), "token-a", "kitchen-v0", Behaviour::default()),
        play_match(url, "token-b", "kitchen-v0", Behaviour::default()),
    );
    assert_eq!(finished(a).start.environment, "kitchen-v0");
    finished(b);

    // Personne d'autre ne demandait le football : A y attend toujours.
    assert!(!football.is_finished());
    football.abort();
}
