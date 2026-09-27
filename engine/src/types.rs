//! Types de données échangés avec l'extérieur du moteur : actions en entrée
//! de `step()`, observations/reward/info en sortie.
//!
//! Tous dérivent `Serialize`/`Deserialize` (serde) pour être réutilisés tels
//! quels par le crate `protocol` et encodés en MessagePack sur le réseau.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// Actions (entrée de step())
// ---------------------------------------------------------------------

/// Action d'UN seul joueur pour un tick.
///
/// `#[derive(Copy)]` : cette struct ne contient que des `f32`/`Option<f32-tuple>`
/// (pas de `Vec`, pas de `String`), donc elle est bon marché à dupliquer bit à
/// bit. On peut donc la marquer `Copy` : elle se comporte comme un `int` en
/// C, pas besoin de `.clone()` explicite, une affectation la duplique.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Action {
    /// Direction de déplacement souhaitée : vecteur `(dx, dy)`, idéalement de
    /// norme <= 1.0 (le moteur clampe si besoin). Le moteur applique une
    /// vitesse/force dans cette direction lors du prochain `step`.
    pub move_dir: (f32, f32),

    /// Impulsion sur le ballon si celui-ci est à portée : `(dx, dy)`, la
    /// direction du tir/passe, avec la puissance encodée dans la norme du
    /// vecteur. `None` = pas de tir ce tick.
    ///
    /// En Rust, `Option<T>` remplace les conventions "valeur sentinelle"
    /// utilisées en C (comme `-1` ou `NULL`) : soit `Some((dx, dy))`, soit
    /// `None`, et le compilateur oblige à gérer les deux cas explicitement
    /// (pas d'oubli possible, contrairement à un pointeur NULL non vérifié).
    pub kick: Option<(f32, f32)>,
}

impl Action {
    /// Action neutre : aucun déplacement, aucun tir. Utilisée comme valeur
    /// par défaut côté serveur si un agent ne répond pas dans les temps.
    ///
    /// `const` (et pas juste une fonction) : cette valeur est calculable au
    /// moment de la compilation, on peut donc l'utiliser comme constante,
    /// ex: `let a = Action::NOOP;` sans coût à l'exécution.
    pub const NOOP: Action = Action {
        move_dir: (0.0, 0.0),
        kick: None,
    };
}

/// Actions de tous les joueurs d'UNE équipe pour un tick.
///
/// `Vec<Action>` (tableau de taille dynamique, comme une `list` Python ou un
/// `malloc` redimensionnable en C, mais avec gestion mémoire automatique) et
/// non un tableau de taille fixe, car `players_per_team` est décidé au
/// runtime par `EngineConfig`, pas au moment de la compilation.
///
/// Convention : `players[i]` correspond au joueur `i` dans
/// `Observation::self_team[i]` de la même équipe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actions {
    pub players: Vec<Action>,
}

// ---------------------------------------------------------------------
// Observation (sortie de step()/reset())
// ---------------------------------------------------------------------

/// État d'un joueur (coéquipier ou adversaire) tel que vu dans une Observation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PlayerObs {
    /// Position normalisée dans `[-1.0, 1.0] x [-1.0, 1.0]`, origine au
    /// centre du terrain. `-1.0`/`1.0` en X correspondent aux deux lignes de
    /// but, `-1.0`/`1.0` en Y aux deux lignes de touche.
    pub position: (f32, f32),
    /// Vitesse normalisée (même échelle que `position`, par seconde).
    pub velocity: (f32, f32),
}

/// État du ballon tel que vu dans une Observation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BallObs {
    pub position: (f32, f32),
    pub velocity: (f32, f32),
}

/// Observation renvoyée à UNE équipe après un `reset()`/`step()`.
///
/// Point le plus important : cette observation est TOUJOURS exprimée du
/// point de vue de l'équipe qui la reçoit, quel que soit le côté physique du
/// terrain sur lequel elle joue réellement en interne. Concrètement, le
/// moteur "retourne" (mirror : `x -> -x`, `vx -> -vx`) les coordonnées pour
/// l'équipe qui joue à droite, de sorte que dans CETTE struct :
/// - `self_team` contient toujours les propres joueurs du destinataire ;
/// - le but à ATTAQUER est toujours du côté `x = +1.0` ;
/// - le but à DÉFENDRE est toujours du côté `x = -1.0`.
///
/// Un agent ne peut donc jamais déduire, à partir de l'observation seule,
/// s'il joue physiquement à gauche ou à droite du terrain réel — condition
/// demandée pour que le même modèle soit valide des deux côtés.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// Les joueurs de l'équipe destinataire, dans l'ordre attendu par
    /// `Actions::players` au tick suivant (self_team[i] <-> players[i]).
    pub self_team: Vec<PlayerObs>,
    /// Les joueurs de l'équipe adverse, vus depuis le même référentiel miroir.
    pub opponent_team: Vec<PlayerObs>,
    pub ball: BallObs,
    /// `(buts marqués par soi, buts encaissés)` depuis le début du match.
    pub score: (u32, u32),
    /// Nombre de ticks restants avant fin de match par limite de temps.
    pub ticks_remaining: u32,
}

// ---------------------------------------------------------------------
// SpectatorFrame (diffusion live / enregistrement des replays)
// ---------------------------------------------------------------------

/// Position/vitesse ABSOLUE (repère réel du terrain, jamais mirroré) d'UN
/// joueur, avec son équipe.
///
/// Différence avec `PlayerObs` : `PlayerObs` est pensé pour un agent (pas de
/// notion d'équipe explicite, coordonnées mirrorées selon le destinataire).
/// `SpectatorPlayer` est pensé pour un humain qui regarde le match : il faut
/// savoir qui est qui, et voir le vrai terrain, pas une version retournée.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SpectatorPlayer {
    /// `0` ou `1`, la même convention que partout ailleurs dans `Engine`
    /// (équipe 0 défend `x = -1`, équipe 1 défend `x = +1`, dans CE
    /// référentiel non mirroré — à ne pas confondre avec l'indexation
    /// `self_team`/`opponent_team` d'une `Observation`, qui dépend du
    /// destinataire).
    pub team: u8,
    pub position: (f32, f32),
    pub velocity: (f32, f32),
}

/// État complet du match à un tick donné, dans le référentiel RÉEL du
/// terrain (jamais mirroré).
///
/// Utilisé pour la diffusion en direct aux spectateurs et pour
/// l'enregistrement des replays — jamais envoyé à un agent. C'est
/// exactement la distinction qui existe entre "l'état du monde" et
/// "ce qu'un joueur en particulier en perçoit" : `Observation` est la
/// perception (mirrorée, relative), `SpectatorFrame` est l'état (absolu).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectatorFrame {
    pub tick: u32,
    pub players: Vec<SpectatorPlayer>,
    pub ball: BallObs,
    /// `(buts équipe 0, buts équipe 1)`, jamais réordonné (contrairement à
    /// `Observation::score`, qui est toujours `(soi, adversaire)`).
    pub score: (u32, u32),
}

// ---------------------------------------------------------------------
// Reward / Done / Info
// ---------------------------------------------------------------------

/// Récompense d'UNE équipe à l'issue d'un `step()`.
///
/// C'est volontairement une struct dédiée, et pas un simple `f32`, pour
/// pouvoir ajouter plus tard des composantes de "reward shaping" (ex: bonus
/// de possession, pénalité de distance au ballon...) sans casser la
/// signature de `Engine::step`. Pour l'instant seules `goal_scored` et
/// `goal_conceded` existent, et `total()` fait juste leur somme.
///
/// `#[derive(Default)]` génère une implémentation qui met tous les champs à
/// leur valeur par défaut (`0.0` pour `f32`), utile pour dire "reward nulle
/// ce tick" sans tout écrire à la main.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Reward {
    /// `1.0` si cette équipe a marqué CE tick, `0.0` sinon.
    pub goal_scored: f32,
    /// `-1.0` si cette équipe a encaissé CE tick, `0.0` sinon.
    pub goal_conceded: f32,
}

impl Reward {
    /// Valeur scalaire consommée par un agent RL classique. Les champs
    /// détaillés restent disponibles séparément pour du debug/logging, ou
    /// pour des variantes de reward shaping côté entraînement.
    pub fn total(&self) -> f32 {
        self.goal_scored + self.goal_conceded
    }
}

/// `true` quand le match est terminé (limite de ticks atteinte ou score max
/// atteint par une des deux équipes). Simple alias de lisibilité : `type X = Y`
/// ne crée pas un nouveau type, juste un autre nom pour `bool` à cet endroit.
pub type Done = bool;

/// Métadonnées additionnelles non essentielles à la décision d'un agent,
/// utiles pour le debug/logging côté serveur (ex: `tracing::debug!`).
/// Minimal en V1, mais laissé extensible (hors-jeu, fautes... plus tard).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Info {
    pub tick: u32,
}

// ---------------------------------------------------------------------
// Résultats agrégés de reset()/step() (2 équipes)
// ---------------------------------------------------------------------

/// Résultat d'un `Engine::step`, pour les DEUX équipes à la fois.
///
/// `[Observation; 2]` est un TABLEAU de taille fixe (2), pas un `Vec` : en
/// Rust, `[T; N]` a sa taille connue à la compilation (ici toujours 2 équipes
/// par match), donc pas d'allocation sur le tas contrairement à `Vec`. On
/// utilise l'index `0` pour l'équipe A et `1` pour l'équipe B — cette
/// convention est purement interne au moteur/serveur, elle n'apparaît jamais
/// dans l'Observation elle-même (qui est déjà relative/symétrique).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub observations: [Observation; 2],
    pub rewards: [Reward; 2],
    /// Partagé : le match se termine pour les deux équipes en même temps.
    pub done: Done,
    pub info: Info,
}
