//! État partagé du gateway : file d'attente de matchmaking et fournisseur
//! d'authentification, injectés dans les handlers axum.

use axum::extract::ws::WebSocket;
use engine::types::SpectatorFrame;
use engine::EngineConfig;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

use crate::auth::{AuthProvider, PlayerId};
use crate::match_runner::{MatchId, MatchRunnerConfig};
use crate::matches::MatchStore;

/// Un joueur authentifié qui attend un adversaire dans la file FIFO.
///
/// `socket` est déplacé (move) dans la struct : `PendingPlayer` en devient
/// l'unique propriétaire tant qu'il reste en file. Quand on `pop_front` ce
/// joueur pour démarrer un match, `socket` est à nouveau déplacé vers
/// `run_match` — aucune copie, aucun partage, exactement UN propriétaire à
/// la fois. C'est le modèle d'ownership de Rust : il rend impossible d'agir
/// par erreur sur un socket depuis deux endroits en même temps.
pub struct PendingPlayer {
    pub id: PlayerId,
    pub socket: WebSocket,
}

/// État partagé de l'application, cloné (bon marché, cf. `Arc`) dans chaque
/// handler axum via l'extracteur `State<AppState>`.
///
/// - `Arc<T>` ("Atomic Reference Counted") permet à plusieurs tâches tokio
///   (une par connexion WebSocket) de partager la MÊME donnée sans la copier
///   et sans durée de vie explicite à gérer (pas de `free()`/`drop()` manuel
///   comme en C : la donnée est libérée automatiquement quand le dernier
///   `Arc` pointant vers elle est détruit).
/// - `std::sync::Mutex` (pas `tokio::sync::Mutex`) : on ne fait jamais
///   d'opération asynchrone (`.await`) pendant que le verrou est tenu (juste
///   un `push_back`/`pop_front` synchrone sur la `VecDeque`), donc le mutex
///   standard — plus léger — suffit et est préférable au mutex async de
///   tokio dans ce cas précis.
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<dyn AuthProvider>,
    /// Persistance des résultats/replays de match (Postgres si `DATABASE_URL`
    /// est configurée, no-op sinon — cf. `crate::matches`).
    pub match_store: Arc<dyn MatchStore>,
    pub matchmaking_queue: Arc<Mutex<VecDeque<PendingPlayer>>>,

    /// Configuration appliquée à CHAQUE nouveau match. Fixée une fois au
    /// démarrage du serveur (cf. `main.rs`) plutôt que codée en dur dans le
    /// gateway : ça permet par exemple à un test de démarrer des matchs
    /// très courts (peu de `max_ticks`) sans dupliquer toute la logique de
    /// matchmaking.
    pub engine_config: EngineConfig,
    pub match_config: MatchRunnerConfig,

    /// Registre des matchs EN COURS, pour l'endpoint spectateur
    /// (`/spectate/:match_id`) : chaque match en cours a une entrée, ajoutée
    /// à sa création et retirée à sa fin (cf. `handle_new_connection`). Un
    /// nouveau spectateur n'a qu'à récupérer le `Sender` correspondant et
    /// appeler `.subscribe()` pour recevoir les frames à partir de
    /// maintenant (les `broadcast::Sender` ne rejouent jamais les messages
    /// passés à un nouvel abonné : un spectateur qui arrive en cours de
    /// match voit les frames suivantes, pas l'historique).
    ///
    /// Même raisonnement que `matchmaking_queue` pour `std::sync::Mutex` :
    /// insertion/lecture/suppression sont toujours synchrones, jamais
    /// d'`.await` verrou tenu.
    pub spectators: Arc<Mutex<HashMap<MatchId, broadcast::Sender<SpectatorFrame>>>>,
}
