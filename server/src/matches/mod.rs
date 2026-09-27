//! Persistance des matchs (métadonnées + replay), pour que le site web
//! (`frondori-web`, MÊME base Postgres, cf. `CLAUDE.md`) puisse lister les
//! matchs en direct et servir les replays plus tard.
//!
//! Même esprit que `crate::auth` : une abstraction (`MatchStore`), une
//! implémentation Postgres (`postgres::PostgresMatchStore`), et une
//! implémentation "aucune persistance" (`NullMatchStore`) pour le mode sans
//! base — les matchs restent jouables normalement, juste non enregistrés.

pub mod postgres;

use async_trait::async_trait;
use engine::types::SpectatorFrame;

pub use postgres::PostgresMatchStore;

use crate::match_runner::MatchId;

/// Nom de l'environnement de jeu, tel qu'enregistré en base. En dur pour
/// l'instant : ce serveur n'implémente qu'un seul jeu (football). À
/// paramétrer si/quand un second environnement existe (cf. la maquette
/// `frondori-web`, qui anticipe déjà cette idée).
pub const ENVIRONMENT: &str = "football";

/// Abstraction sur la persistance des matchs. `Send + Sync` : même raison
/// que `AuthProvider`, un `Arc<dyn MatchStore>` est partagé entre toutes les
/// tâches tokio (une par match).
#[async_trait]
pub trait MatchStore: Send + Sync {
    /// Enregistre le DÉBUT d'un match (statut "live"), avant le premier
    /// tick. Ne doit jamais empêcher le match de démarrer : une erreur de
    /// persistance est loggée en interne (cf. implémentations), jamais
    /// remontée à l'appelant — la persistance est un "bonus", pas une
    /// condition pour jouer.
    async fn record_start(&self, match_id: MatchId, home_player_id: &str, away_player_id: &str);

    /// Enregistre la FIN d'un match : score final, résultat du point de vue
    /// de l'équipe "home" (= équipe d'indice 0, la même convention que
    /// partout ailleurs dans `engine`/`match_runner`), et la séquence
    /// complète de frames pour permettre un replay plus tard (aucune vidéo
    /// n'est jamais enregistrée — uniquement ces données).
    async fn record_end(
        &self,
        match_id: MatchId,
        home_score: u32,
        away_score: u32,
        home_outcome: protocol::MatchOutcome,
        replay: &[SpectatorFrame],
    );
}

/// Implémentation "aucune persistance", utilisée quand `DATABASE_URL` n'est
/// pas configurée (cf. `main.rs`).
pub struct NullMatchStore;

#[async_trait]
impl MatchStore for NullMatchStore {
    async fn record_start(&self, _match_id: MatchId, _home_player_id: &str, _away_player_id: &str) {
    }

    async fn record_end(
        &self,
        _match_id: MatchId,
        _home_score: u32,
        _away_score: u32,
        _home_outcome: protocol::MatchOutcome,
        _replay: &[SpectatorFrame],
    ) {
    }
}
