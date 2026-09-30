//! `protocol` : types de messages échangés entre le serveur (gateway / match
//! runner) et les SDK participants, plus les fonctions d'encodage/décodage
//! MessagePack qui définissent le format exact sur le fil.
//!
//! Ce protocole ne connaît AUCUN jeu. Observations et actions sont des
//! valeurs MessagePack opaques (`Value`) dont la forme dépend de
//! l'environnement joué : c'est lui qui la décrit (ses spaces, envoyés dans
//! `MatchStart`) et qui valide chaque action reçue (côté worker Python, cf.
//! `frondori_engine/wire.py`). Ajouter un environnement ne demande donc
//! jamais de modifier ce crate.
//!
//! Convention inchangée : UN socket = UN agent de l'environnement (une
//! équipe entière au football, un chef en cuisine).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Valeur MessagePack arbitraire (nombre, liste, map...). Ré-exportée pour
/// que le serveur et les tests n'aient pas à dépendre de `rmpv` eux-mêmes.
pub use rmpv::Value;

// ---------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------

/// Premier message envoyé par le client juste après l'ouverture du
/// WebSocket. Le serveur n'accepte aucun autre message tant que celui-ci n'a
/// pas été reçu et validé.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    /// Jeton d'authentification du participant (de son agent).
    pub token: String,
    /// Environnement que l'agent veut jouer (ex: `"kitchen-v0"`). Un même
    /// agent peut jouer à plusieurs environnements : c'est le client qui
    /// choisit, à chaque connexion, et le classement est tenu par couple
    /// (agent, environnement).
    pub environment: String,
    /// Nom du modèle/SDK, informatif uniquement (logs, affichage).
    pub client_name: String,
}

/// Réponse du serveur à un `Hello` valide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Welcome {
    pub player_id: String,
    /// Environnement dans lequel le participant est mis en file (celui
    /// demandé dans `Hello`).
    pub environment: String,
}

/// Réponse du serveur à un `Hello` refusé (token inconnu, environnement
/// indisponible sur ce serveur...). Le serveur ferme le socket juste après.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthError {
    pub reason: String,
}

// ---------------------------------------------------------------------
// Boucle de match
// ---------------------------------------------------------------------

/// Envoyé à chaque participant juste après l'appariement, avant la première
/// `ObservationMessage`.
///
/// `observation_space`/`action_space` décrivent la forme des valeurs de CE
/// match pour CET agent (format : `frondori_engine.wire.space_to_spec`,
/// ex. `{"type": "discrete", "n": 6, "start": 0}`). C'est ce qui permet à un
/// SDK générique de décoder les observations et de construire des actions
/// valides sans rien connaître du jeu à l'avance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchStart {
    /// Identifiant public du match (UUID en texte), à passer tel quel, par
    /// exemple pour suivre le flux spectateur `/spectate/<match_id>`.
    pub match_id: String,
    pub environment: String,
    /// Nom de l'agent contrôlé par ce participant (ex: `"team_0"`).
    pub agent: String,
    /// Tous les agents du match, dans l'ordre de l'environnement.
    pub agents: Vec<String>,
    pub observation_space: Value,
    pub action_space: Value,
}

/// Action de l'agent pour un tick. Une action absente (délai dépassé) ou
/// invalide pour l'`action_space` est remplacée par l'action neutre de
/// l'environnement : jamais une erreur fatale pour le match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionMessage {
    /// Numéro du tick auquel cette action répond, pour permettre au client
    /// de détecter un décalage.
    pub tick: u32,
    pub action: Value,
}

/// Ce qu'il est advenu de l'action que l'agent devait envoyer pour le tick
/// précédent. Sans ce retour, un agent dont toutes les actions sont
/// refusées (mauvais format) ou arrivent trop tard jouerait l'action neutre
/// pendant toute la partie sans jamais le savoir.
///
/// Enum SANS données : encodé comme la simple chaîne du nom de la variante
/// (`"Applied"`), pas comme une map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionStatus {
    /// Pas d'action attendue (observation initiale du match).
    NotExpected,
    Applied,
    /// Reçue, mais invalide pour l'`action_space` : l'action neutre a été
    /// appliquée à la place.
    Rejected,
    /// Pas reçue à temps : l'action neutre a été appliquée.
    Missing,
}

/// Observation de l'agent pour un tick, avec la récompense qu'il vient de
/// recevoir (0 au tick initial).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationMessage {
    pub tick: u32,
    pub observation: Value,
    /// Sort de l'action envoyée pour le tick précédent.
    pub last_action: ActionStatus,
    pub reward: f64,
    /// Fin de partie pour cet agent (au sens Gymnasium : l'épisode est fini).
    pub terminated: bool,
    /// Coupure par limite de temps.
    pub truncated: bool,
    /// Informations annexes de l'environnement (debug, statistiques).
    pub info: Value,
}

/// Fin de match, envoyée une seule fois juste avant la fermeture du socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchEnd {
    /// Somme des récompenses de chaque agent sur tout le match. Un
    /// vainqueur, un score commun... : c'est au client d'en tirer ce qui a
    /// du sens pour l'environnement (compétitif ou coopératif).
    pub returns: HashMap<String, f64>,
    /// Agents dont le participant s'est déconnecté en cours de match (le
    /// match s'arrête alors immédiatement, pas de reconnexion).
    pub forfeited: Vec<String>,
}

// ---------------------------------------------------------------------
// Latence / monitoring
// ---------------------------------------------------------------------

/// Envoyé par le serveur pour vérifier qu'un client est toujours là.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Ping {
    /// Valeur arbitraire renvoyée telle quelle dans le `Pong` correspondant.
    pub nonce: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Pong {
    pub nonce: u64,
}

// ---------------------------------------------------------------------
// Enveloppes
// ---------------------------------------------------------------------

/// Tous les messages que le SERVEUR peut envoyer à un client.
///
/// Un `enum` Rust peut porter des données différentes par variante
/// (contrairement à un `enum` C, qui n'est qu'un entier nommé) : c'est
/// l'équivalent type-safe d'un "union taggé", et un `match` sur ce type
/// oblige à gérer chaque variante. Encodé en map à une seule clé :
/// `{"Welcome": {...}}` (représentation "externally tagged" de serde).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    Welcome(Welcome),
    AuthError(AuthError),
    MatchStart(MatchStart),
    Observation(ObservationMessage),
    MatchEnd(MatchEnd),
    Ping(Ping),
}

/// Tous les messages que le CLIENT peut envoyer au serveur.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello(Hello),
    Action(ActionMessage),
    Pong(Pong),
}

// ---------------------------------------------------------------------
// Encodage / décodage MessagePack
// ---------------------------------------------------------------------

/// Erreur d'encodage ou de décodage d'un message.
#[derive(Debug)]
pub struct CodecError(pub String);

/// Encode un message en MessagePack.
///
/// `rmp_serde::to_vec_named` (pas `to_vec`) : les structs sont encodées comme
/// des MAPS `{"champ": valeur}` plutôt que des tableaux positionnels, pour
/// qu'un SDK dans un autre langage décode par nom de champ sans dépendre de
/// l'ordre de déclaration côté Rust.
pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, CodecError> {
    rmp_serde::to_vec_named(message).map_err(|e| CodecError(e.to_string()))
}

/// Décode un message MessagePack précédemment produit par [`encode`].
pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, CodecError> {
    rmp_serde::from_slice(bytes).map_err(|e| CodecError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une `Value` imbriquée dans un message doit survivre à l'aller-retour
    /// MessagePack à l'identique : c'est tout ce que le serveur fait des
    /// observations et des actions (les relayer sans les comprendre).
    #[test]
    fn opaque_values_survive_a_round_trip() {
        let observation = Value::Map(vec![(
            Value::from("self_team"),
            Value::Array(vec![Value::from(0.5_f64), Value::from(-1.0_f64)]),
        )]);
        let message = ServerMessage::Observation(ObservationMessage {
            tick: 7,
            observation: observation.clone(),
            last_action: ActionStatus::Rejected,
            reward: 1.0,
            terminated: false,
            truncated: true,
            info: Value::Map(vec![]),
        });

        let decoded: ServerMessage = decode(&encode(&message).unwrap()).unwrap();

        let ServerMessage::Observation(decoded) = decoded else {
            panic!("variante inattendue");
        };
        assert_eq!(decoded.tick, 7);
        assert_eq!(decoded.observation, observation);
        assert_eq!(decoded.last_action, ActionStatus::Rejected);
        assert!(decoded.truncated);
    }

    /// Un enum sans données doit arriver comme une simple chaîne : c'est ce
    /// que les SDK (Python...) décodent.
    #[test]
    fn action_status_is_encoded_as_a_plain_string() {
        let bytes = encode(&ActionStatus::Missing).unwrap();
        let raw: Value = decode(&bytes).unwrap();
        assert_eq!(raw, Value::from("Missing"));
    }
}
