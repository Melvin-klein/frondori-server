//! État partagé du gateway, injecté dans les handlers axum.

use axum::extract::ws::WebSocket;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

use crate::auth::{AuthProvider, PlayerId};
use crate::environments::{Catalog, WorkerCommand};
use crate::match_runner::{MatchId, MatchRunnerConfig};
use crate::matches::MatchStore;

/// Un joueur authentifié qui attend ses partenaires/adversaires en file.
///
/// `socket` est déplacé (move) dans la struct : `PendingPlayer` en devient
/// l'unique propriétaire tant qu'il reste en file, puis il est à nouveau
/// déplacé vers `run_match` au démarrage du match — exactement UN
/// propriétaire à la fois, jamais de socket manipulé depuis deux endroits.
pub struct PendingPlayer {
    pub id: PlayerId,
    pub socket: WebSocket,
}

/// État partagé de l'application, cloné (bon marché, cf. `Arc`) dans chaque
/// handler axum via l'extracteur `State<AppState>`.
///
/// - `Arc<T>` ("Atomic Reference Counted") permet à plusieurs tâches tokio
///   de partager la MÊME donnée sans la copier ; elle est libérée
///   automatiquement quand le dernier `Arc` disparaît.
/// - `std::sync::Mutex` (pas `tokio::sync::Mutex`) : on ne fait jamais
///   d'opération asynchrone (`.await`) pendant que le verrou est tenu, donc
///   le mutex standard, plus léger, suffit.
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<dyn AuthProvider>,
    /// Persistance des matchs (Postgres si `DATABASE_URL` est configurée,
    /// no-op sinon — cf. `crate::matches`).
    pub match_store: Arc<dyn MatchStore>,

    /// Environnements disponibles sur ce serveur, décrits une fois au
    /// démarrage par un worker (cf. `main.rs`). Lecture seule ensuite.
    pub catalog: Arc<Catalog>,
    /// Commande qui lance un worker, une fois par match.
    pub worker_command: WorkerCommand,

    /// Une file d'attente PAR environnement : on n'apparie jamais un agent
    /// de football avec un chef cuisinier.
    pub matchmaking_queues: Arc<Mutex<HashMap<String, VecDeque<PendingPlayer>>>>,
    pub match_config: MatchRunnerConfig,

    /// Registre des matchs EN COURS, pour l'endpoint spectateur
    /// (`/spectate/:match_id`) : une entrée ajoutée à la création du match
    /// et retirée à sa fin. Un nouveau spectateur récupère le `Sender` et
    /// appelle `.subscribe()` pour recevoir les scènes à partir de
    /// maintenant (un `broadcast` ne rejoue jamais l'historique).
    pub spectators: Arc<Mutex<HashMap<MatchId, broadcast::Sender<Arc<str>>>>>,
}
