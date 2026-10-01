//! Une action arrivée APRÈS le délai de son tick ne doit jamais être
//! appliquée à un tick suivant. Bug réel trouvé en réseau : le serveur
//! prenait le prochain message du socket sans regarder son `tick`, si bien
//! qu'après un seul retard, TOUTES les actions de l'agent étaient appliquées
//! avec un pas de décalage jusqu'à la fin du match (une politique scriptée
//! de cuisine servait 13 soupes en local, 7 en réseau, puis se bloquait).

mod common;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{ActionStatus, ClientMessage, ServerMessage, Value};
use server::matches::NullMatchStore;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use common::{play_match, Behaviour, Outcome};

const STAY: u64 = 0;
const MOVE_DOWN: u64 = 2;

/// Client de cuisine qui répond à l'observation initiale bien après le
/// délai, par un déplacement vers le bas, puis reste immobile (`STAY`) à
/// chaque tick suivant, en répondant tout de suite. Renvoie la position
/// `(ligne, colonne)` de son chef dans chaque observation reçue, et le
/// statut de son action rapporté par l'observation suivante (tick 1).
async fn late_then_still(url: String, token: &str) -> (Vec<(u64, u64)>, Option<ActionStatus>) {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let hello = ClientMessage::Hello(protocol::Hello {
        token: token.to_string(),
        environment: "kitchen-v0".to_string(),
        client_name: "late-client".to_string(),
    });
    ws.send(WsMessage::Binary(protocol::encode(&hello).unwrap())).await.unwrap();

    let (mut positions, mut status_after_late) = (Vec::new(), None);
    loop {
        let Some(Ok(WsMessage::Binary(bytes))) = ws.next().await else {
            panic!("connexion fermée avant la fin du match");
        };
        match protocol::decode::<ServerMessage>(&bytes).unwrap() {
            ServerMessage::Ping(ping) => {
                let pong = ClientMessage::Pong(protocol::Pong { nonce: ping.nonce });
                ws.send(WsMessage::Binary(protocol::encode(&pong).unwrap())).await.unwrap();
            }
            ServerMessage::Observation(observation) => {
                let me = common::field(&observation.observation, "self").and_then(Value::as_array).unwrap();
                positions.push((me[0].as_u64().unwrap(), me[1].as_u64().unwrap()));
                if observation.tick == 1 {
                    status_after_late = Some(observation.last_action);
                }
                if observation.terminated || observation.truncated {
                    continue;
                }
                let action = if observation.tick == 0 {
                    // Largement après le délai (50 ms) : plusieurs ticks passent.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    MOVE_DOWN
                } else {
                    STAY
                };
                let message = ClientMessage::Action(protocol::ActionMessage {
                    tick: observation.tick,
                    action: Value::from(action),
                });
                ws.send(WsMessage::Binary(protocol::encode(&message).unwrap())).await.unwrap();
            }
            ServerMessage::MatchEnd(_) => return (positions, status_after_late),
            _ => {}
        }
    }
}

#[tokio::test]
async fn a_late_action_is_dropped_instead_of_being_applied_to_a_later_tick() {
    // 100 pas/s : le délai d'action est alors le minimum de 50 ms.
    let addr = common::start_test_server_with(std::sync::Arc::new(NullMatchStore), 100.0).await;
    let url = format!("ws://{addr}/agent");

    let (late, other) = tokio::join!(
        late_then_still(url.clone(), "token-a"),
        play_match(url, "token-b", "kitchen-v0", Behaviour::default()),
    );
    assert!(matches!(other, Outcome::Finished(_)));
    let (positions, status_after_late) = late;

    assert_eq!(status_after_late, Some(ActionStatus::Missing));
    // Le déplacement arrivé en retard n'a jamais été joué : le chef n'a pas
    // bougé de tout le match (toutes ses autres actions sont `STAY`).
    let start = positions[0];
    assert!(
        positions.iter().all(|&position| position == start),
        "le chef a bougé ({start:?} -> {:?}) : une action en retard a été appliquée à un autre tick",
        positions.iter().find(|&&position| position != start),
    );
    assert_eq!(positions.len(), 201);
}
