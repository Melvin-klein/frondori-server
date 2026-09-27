//! `protocol` : types de messages échangés entre le serveur (module gateway
//! / match runner) et les SDK participants, plus les fonctions d'encodage/
//! décodage MessagePack qui définissent le format exact sur le fil.
//!
//! Rappel de convention (validée avant l'implémentation) : UN socket
//! représente UNE équipe entière. `ActionMessage` transporte donc les actions
//! de tous les joueurs de l'équipe (`engine::types::Actions`), pas d'un seul
//! joueur.

use engine::types::{Actions, Observation};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------

/// Premier message envoyé par le client juste après l'ouverture du
/// WebSocket. Le serveur n'accepte aucun autre message tant que celui-ci n'a
/// pas été reçu et validé.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    /// Jeton d'authentification du participant, vérifié côté serveur via le
    /// trait `AuthProvider` (crate `server`).
    pub token: String,
    /// Nom du modèle/SDK, informatif uniquement (logs, affichage).
    pub client_name: String,
}

/// Réponse du serveur à un `Hello` valide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Welcome {
    /// Identifiant attribué à ce joueur pour la durée de la connexion.
    pub player_id: String,
}

/// Réponse du serveur à un `Hello` invalide (token inconnu/expiré). Le
/// serveur ferme le socket juste après avoir envoyé ce message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthError {
    pub reason: String,
}

// ---------------------------------------------------------------------
// Boucle de match
// ---------------------------------------------------------------------

/// Envoyé aux deux équipes juste après l'appariement (matchmaking), avant la
/// première `ObservationMessage` : donne l'identifiant public du match.
///
/// `match_id` est une simple `String` ici (pas de type `Uuid` importé dans
/// ce crate) : le crate `protocol` doit rester consommable tel quel par un
/// SDK dans n'importe quel langage, `match_id` n'est qu'une valeur opaque à
/// afficher/transmettre, jamais à parser. Sert notamment à corréler ce
/// match avec le flux spectateur (`/spectate/<match_id>` côté serveur, en
/// JSON — un protocole séparé, cf. `server::gateway`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchStart {
    pub match_id: String,
}

/// Actions de toute l'équipe, envoyées par le client à chaque tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionMessage {
    /// Numéro de tick auquel cette action s'applique, pour permettre au
    /// client de détecter un décalage/une perte de synchronisation.
    pub tick: u32,
    pub actions: Actions,
}

/// Observation envoyée par le serveur à chaque tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservationMessage {
    pub tick: u32,
    pub observation: Observation,
}

/// Issue du match du point de vue du destinataire de ce message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchOutcome {
    Win,
    Loss,
    Draw,
    /// L'adversaire s'est déconnecté avant la fin du match : victoire par
    /// forfait (cf. contrainte "pas de reconnexion automatique").
    OpponentForfeit,
}

/// Message de fin de match, envoyé une seule fois juste avant la fermeture
/// propre du socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchEnd {
    pub outcome: MatchOutcome,
    /// `(buts marqués par le destinataire, buts encaissés)`.
    pub final_score: (u32, u32),
}

// ---------------------------------------------------------------------
// Latence / monitoring
// ---------------------------------------------------------------------

/// Envoyé par le serveur pour mesurer la latence aller-retour d'un client.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Ping {
    /// Valeur arbitraire (ex: horodatage) renvoyée telle quelle dans le `Pong`
    /// correspondant, pour associer chaque réponse à sa requête.
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
/// Un `enum` Rust peut porter des données différentes par variante (contrairement
/// à un `enum` C, qui n'est qu'un entier nommé) : c'est l'équivalent
/// type-safe d'un "union taggé". `ServerMessage::Welcome(Welcome { .. })` est
/// une valeur à part entière, et un `match` sur ce type oblige à gérer
/// chaque variante (le compilateur refuse de compiler s'il en manque une).
///
/// Pas de `#[serde(tag = "...")]` ici volontairement : la représentation par
/// défaut de serde pour les enums ("externally tagged", encodée par
/// `rmp-serde` comme un tableau MessagePack `[index_de_variante, données]`)
/// est plus compacte et plus rapide à (dé)sérialiser que le "tag interne"
/// (qui a besoin de pouvoir bufferiser/relire les champs, pensé surtout pour
/// des formats textuels comme JSON).
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
///
/// `#[from]` n'existe pas nativement ici (pas de `thiserror` dans cette V1
/// pour rester minimal) : on enveloppe juste le message d'erreur en `String`.
/// À remplacer par un enum d'erreurs plus précis si le besoin s'en fait
/// sentir plus tard.
#[derive(Debug)]
pub struct CodecError(pub String);

/// Encode un message en MessagePack.
///
/// `T: Serialize` = contrainte générique : cette fonction accepte n'importe
/// quel type qui implémente `Serialize`, pas seulement `ServerMessage`/
/// `ClientMessage`. En pratique, seuls ces deux enums seront passés ici côté
/// serveur.
///
/// `rmp_serde::to_vec_named` (pas `to_vec`) : les structs sont encodées comme
/// des MAPS `{"champ": valeur}` plutôt que des tableaux positionnels
/// `[valeur, valeur, ...]`. Un peu plus verbeux sur le fil, mais le format
/// devient auto-descriptif — indispensable pour qu'un SDK dans un autre
/// langage (Python, notamment) puisse décoder par nom de champ (`data["player_id"]`)
/// sans dépendre de l'ordre exact de déclaration des champs côté Rust.
/// Les enums (`ServerMessage`, `ClientMessage`) sont, eux, TOUJOURS encodés
/// en map à une seule clé `{"NomDuVariant": contenu}`, indépendamment de ce
/// choix (c'est la représentation "externally tagged" par défaut de serde
/// pour les variantes à données, cf. la doc de `ServerMessage`).
pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, CodecError> {
    rmp_serde::to_vec_named(message).map_err(|e| CodecError(e.to_string()))
}

/// Décode un message MessagePack précédemment produit par [`encode`].
///
/// Fonctionne aussi bien sur un encodage "map" (celui produit par `encode`)
/// que sur un éventuel encodage positionnel : `rmp_serde` détecte le marqueur
/// MessagePack réellement présent sur le fil (tableau ou map) plutôt que de
/// supposer une convention fixée à l'avance.
pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, CodecError> {
    rmp_serde::from_slice(bytes).map_err(|e| CodecError(e.to_string()))
}
