//! Matchs en pas-à-pas : le serveur attend l'action de chaque agent avant
//! d'avancer. La latence réseau ne coûte donc aucune action ; ce qui est
//! limité, c'est le temps de calcul déclaré avec chaque action (budget de
//! l'environnement : 200 ms pour `kitchen-v0`).
//!
//! Un client de cuisine minimal, piloté par un `Plan` : pour chaque tick, le
//! délai avant de répondre (le "réseau"), l'action, et le temps de calcul
//! déclaré. On observe l'effet réel des actions dans la position du chef.

mod common;

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{ActionStatus, ClientMessage, ServerMessage, Value};
use server::match_runner::MatchRunnerConfig;
use server::matches::NullMatchStore;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use common::{play_match, Behaviour, Outcome};

const STAY: u64 = 0;
const MOVE_DOWN: u64 = 2;

/// Pour un tick : (délai avant de répondre, action, temps de calcul déclaré).
type Plan = fn(u32) -> (Duration, u64, f64);

/// Ce que le client a vu : la position `(ligne, colonne)` de son chef dans
/// chaque observation, et le sort de chacune de ses actions.
struct Seen {
    positions: Vec<(u64, u64)>,
    statuses: Vec<ActionStatus>,
}

async fn kitchen_client(url: String, token: &str, plan: Plan) -> Seen {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let hello = ClientMessage::Hello(protocol::Hello {
        token: token.to_string(),
        environment: "kitchen-v0".to_string(),
        client_name: "lockstep-client".to_string(),
    });
    ws.send(WsMessage::Binary(protocol::encode(&hello).unwrap())).await.unwrap();

    let mut seen = Seen { positions: Vec::new(), statuses: Vec::new() };
    loop {
        let Some(Ok(WsMessage::Binary(bytes))) = ws.next().await else {
            panic!("connexion fermée avant la fin du match");
        };
        match protocol::decode::<ServerMessage>(&bytes).unwrap() {
            ServerMessage::MatchStart(start) => assert_eq!(start.compute_budget_ms, 200.0),
            ServerMessage::Ping(ping) => {
                let pong = ClientMessage::Pong(protocol::Pong { nonce: ping.nonce });
                ws.send(WsMessage::Binary(protocol::encode(&pong).unwrap())).await.unwrap();
            }
            ServerMessage::Observation(observation) => {
                let me = common::field(&observation.observation, "self").and_then(Value::as_array).unwrap();
                seen.positions.push((me[0].as_u64().unwrap(), me[1].as_u64().unwrap()));
                if observation.last_action != ActionStatus::NotExpected {
                    seen.statuses.push(observation.last_action);
                }
                if observation.terminated || observation.truncated {
                    continue;
                }
                let (delay, action, compute_ms) = plan(observation.tick);
                tokio::time::sleep(delay).await;
                let message = ClientMessage::Action(protocol::ActionMessage {
                    tick: observation.tick,
                    action: Value::from(action),
                    compute_ms,
                });
                ws.send(WsMessage::Binary(protocol::encode(&message).unwrap())).await.unwrap();
            }
            ServerMessage::MatchEnd(_) => return seen,
            _ => {}
        }
    }
}

/// Joue un match de cuisine entre `plan` et un partenaire qui ne bouge pas.
async fn play(config: MatchRunnerConfig, plan: Plan) -> Seen {
    let addr = common::start_test_server_with_config(Arc::new(NullMatchStore), config).await;
    let url = format!("ws://{addr}/agent");
    let (seen, partner) = tokio::join!(
        kitchen_client(url.clone(), "token-a", plan),
        play_match(url, "token-b", "kitchen-v0", Behaviour::default()),
    );
    assert!(matches!(partner, Outcome::Finished(_)));
    seen
}

fn fast() -> MatchRunnerConfig {
    MatchRunnerConfig {
        tick_rate_override: Some(1000.0),
        ..MatchRunnerConfig::default()
    }
}

#[tokio::test]
async fn a_slow_network_costs_no_action() {
    // 150 ms de "réseau" sur les premiers ticks (au-delà des 50 ms qui
    // faisaient perdre l'action avant le pas-à-pas), calcul déclaré de 1 ms.
    let seen = play(fast(), |tick| {
        let delay = if tick < 5 { Duration::from_millis(150) } else { Duration::ZERO };
        (delay, if tick == 0 { MOVE_DOWN } else { STAY }, 1.0)
    })
    .await;

    // Le déplacement du tick 0 a bien été joué, au tick 1 : le chef est
    // descendu d'une case et n'a plus bougé.
    assert_eq!(seen.positions[1].0, seen.positions[0].0 + 1);
    assert!(seen.positions[1..].iter().all(|&position| position == seen.positions[1]));
    assert_eq!(seen.statuses.len(), 200);
    assert!(seen.statuses.iter().all(|&status| status == ActionStatus::Applied));
}

#[tokio::test]
async fn an_action_over_the_compute_budget_is_replaced_by_the_neutral_action() {
    // Répond tout de suite, mais déclare 250 ms de calcul (budget : 200 ms).
    let seen = play(fast(), |_| (Duration::ZERO, MOVE_DOWN, 250.0)).await;

    assert!(seen.statuses.iter().all(|&status| status == ActionStatus::TooSlow));
    assert!(
        seen.positions.iter().all(|&position| position == seen.positions[0]),
        "une action hors budget a été jouée"
    );
}

#[tokio::test]
async fn a_response_past_the_network_timeout_is_dropped_not_shifted() {
    // Bug réel corrigé : une action arrivée après le délai restait dans le
    // socket et était prise pour la réponse du tick suivant, si bien que
    // l'agent jouait ensuite avec un pas de retard jusqu'à la fin du match.
    // Ici, le déplacement du tick 0 arrive après le délai réseau (100 ms) :
    // il doit être jeté, jamais joué plus tard.
    let config = MatchRunnerConfig {
        response_timeout: Duration::from_millis(100),
        ..fast()
    };
    let seen = play(config, |tick| {
        if tick == 0 {
            (Duration::from_millis(300), MOVE_DOWN, 1.0)
        } else {
            (Duration::ZERO, STAY, 1.0)
        }
    })
    .await;

    assert_eq!(seen.statuses[0], ActionStatus::Missing);
    assert!(
        seen.positions.iter().all(|&position| position == seen.positions[0]),
        "une action en retard a été appliquée à un autre tick"
    );
}

#[tokio::test]
async fn a_match_goes_at_its_nominal_pace_with_fast_agents() {
    // Bug réel corrigé : la boucle attendait DEUX fois le rythme nominal à
    // chaque pas (un reste de l'ancienne boucle à cadence fixe), et
    // n'écoutait les actions qu'après la première attente — un match durait
    // le double, et le temps de réponse mesuré d'un agent immédiat était
    // gonflé d'une période entière (au point de le déclarer suspect).
    // 200 pas à 50 pas/s : 4 s attendues (8 s avec le bug).
    let config = MatchRunnerConfig {
        tick_rate_override: Some(50.0),
        ..MatchRunnerConfig::default()
    };
    let started = std::time::Instant::now();
    let seen = play(config, |_| (Duration::ZERO, STAY, 1.0)).await;
    let elapsed = started.elapsed();

    assert_eq!(seen.statuses.len(), 200);
    assert!(
        elapsed > Duration::from_millis(3900) && elapsed < Duration::from_millis(6000),
        "200 pas à 50 pas/s ont pris {elapsed:?}"
    );
}
