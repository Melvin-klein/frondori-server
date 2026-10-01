//! Persistance des matchs (participants, résultats, replay), pour que le
//! site web (`frondori-web`, MÊME base Postgres) puisse lister les matchs en
//! direct, servir les replays et calculer les classements.
//!
//! Même esprit que `crate::auth` : une abstraction (`MatchStore`), une
//! implémentation Postgres (`postgres::PostgresMatchStore`), et une
//! implémentation "aucune persistance" (`NullMatchStore`) pour le mode sans
//! base — les matchs restent jouables normalement, juste non enregistrés.

pub mod postgres;

use async_trait::async_trait;

pub use postgres::PostgresMatchStore;

use crate::environments::Catalog;
use crate::match_runner::MatchId;

/// Un participant au match : quel joueur contrôle quel agent de
/// l'environnement. `seat` est la position de l'agent dans l'ordre de
/// l'environnement (0 pour `team_0` / `chef_0`...).
#[derive(Debug, Clone)]
pub struct Participant {
    pub seat: usize,
    pub agent: String,
    pub player_id: String,
}

/// Bilan d'un participant en fin de match.
#[derive(Debug, Clone)]
pub struct ParticipantResult {
    pub seat: usize,
    /// Somme de ses récompenses sur tout le match.
    pub final_return: f64,
    /// Son client s'est déconnecté en cours de match.
    pub forfeited: bool,
    /// Dernières informations annexes renvoyées par l'environnement pour cet
    /// agent (ex. score, statistiques) — propres à chaque environnement.
    pub final_info: serde_json::Value,
    /// Bilan de ses temps de réponse : calcul déclaré, aller-retour réseau,
    /// incohérences (cf. `match_runner::timing::TimingSummary`).
    pub timing: serde_json::Value,
    /// Ses temps pas par pas (calcul déclaré, réponse mesurée), pour la
    /// recherche (cf. `match_runner::timing::SeatTiming::steps`).
    pub step_timings: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchStatus {
    /// Joué jusqu'au bout, ou arrêté par la déconnexion d'un participant
    /// (`forfeited` dit lequel) : un résultat exploitable.
    Finished,
    /// Interrompu par une panne de l'environnement lui-même : aucun
    /// participant n'est en cause, le résultat ne doit pas compter.
    Aborted,
}

impl MatchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchStatus::Finished => "finished",
            MatchStatus::Aborted => "aborted",
        }
    }
}

/// Abstraction sur la persistance des matchs. `Send + Sync` : même raison
/// que `AuthProvider`, un `Arc<dyn MatchStore>` est partagé entre toutes les
/// tâches tokio (une par match).
///
/// Aucune de ces méthodes ne renvoie d'erreur : la persistance est un
/// "bonus", pas une condition pour jouer. Une erreur est loggée en interne
/// (cf. implémentations), jamais remontée au match.
#[async_trait]
pub trait MatchStore: Send + Sync {
    /// Publie le catalogue des environnements au démarrage du serveur : le
    /// site en tire la liste des jeux, leurs spaces et leur type de
    /// classement, sans jamais dupliquer ces informations de son côté.
    async fn record_environments(&self, catalog: &Catalog);

    /// Enregistre le DÉBUT d'un match (statut "live"), avant le premier tick.
    async fn record_start(&self, match_id: MatchId, environment: &str, participants: &[Participant]);

    /// Enregistre la FIN d'un match. `replay_json` : le tableau JSON des
    /// scènes du match, une par pas (aucune vidéo n'est jamais enregistrée).
    async fn record_end(
        &self,
        match_id: MatchId,
        status: MatchStatus,
        results: &[ParticipantResult],
        replay_json: &str,
    );
}

/// Implémentation "aucune persistance", utilisée quand `DATABASE_URL` n'est
/// pas configurée (cf. `main.rs`).
pub struct NullMatchStore;

#[async_trait]
impl MatchStore for NullMatchStore {
    async fn record_environments(&self, _catalog: &Catalog) {}

    async fn record_start(&self, _match_id: MatchId, _environment: &str, _participants: &[Participant]) {}

    async fn record_end(
        &self,
        _match_id: MatchId,
        _status: MatchStatus,
        _results: &[ParticipantResult],
        _replay_json: &str,
    ) {
    }
}
