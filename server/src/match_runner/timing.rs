//! Mesure des temps de réponse d'un participant pendant un match.
//!
//! Les matchs se jouent en pas-à-pas : le serveur attend l'action de chaque
//! agent avant d'avancer, la latence réseau ne coûte donc rien. Ce qui est
//! limité, c'est le temps de CALCUL de l'agent, que le client mesure et
//! déclare avec chaque action (`ActionMessage::compute_ms`).
//!
//! Un temps déclaré n'est pas vérifiable directement (le client tourne chez
//! le participant). Le serveur le confronte donc à ce qu'il mesure lui-même :
//! - le temps de RÉPONSE : de l'envoi de l'observation à la réception de
//!   l'action, soit réseau + calcul ;
//! - l'aller-retour RÉSEAU, par des `Ping` envoyés pendant le match, à un
//!   moment où l'agent n'a rien à calculer.
//!
//! Pour un client honnête, réponse ≈ aller-retour + calcul déclaré. Un écart
//! durable ("temps inexpliqué") signale un temps de calcul sous-déclaré. Le
//! contrôle reste une dissuasion, pas une preuve : un client modifié peut
//! aussi retarder ses `Pong` pour gonfler son aller-retour apparent.
//!
//! Tout est enregistré avec le match : ces temps sont aussi une donnée de
//! recherche (distribution des temps de réponse de chaque modèle).

use serde::Serialize;

/// Au-delà de ce temps inexpliqué médian (ms), les déclarations de temps de
/// calcul d'un participant sont jugées suspectes. Large, pour ne pas accuser
/// un client honnête de la gigue du réseau ou d'une machine chargée.
pub const SUSPECT_UNEXPLAINED_MS: f64 = 50.0;

/// Temps relevés pour un participant, un élément par pas où une action
/// était attendue de lui.
#[derive(Debug, Default)]
pub struct SeatTiming {
    /// Temps de calcul déclaré (`None` : pas d'action reçue, ou déclaration
    /// invalide).
    compute_ms: Vec<Option<f64>>,
    /// Temps de réponse mesuré par le serveur (`None` : pas d'action reçue).
    response_ms: Vec<Option<f64>>,
    /// Allers-retours réseau mesurés par `Ping`.
    rtt_ms: Vec<f64>,
    too_slow: u32,
    missing: u32,
}

impl SeatTiming {
    /// Une action reçue : son temps de calcul déclaré (s'il est valide) et
    /// le temps de réponse mesuré. `too_slow` : déclaration au-delà du budget.
    pub fn record_action(&mut self, compute_ms: Option<f64>, response_ms: f64, too_slow: bool) {
        self.compute_ms.push(compute_ms);
        self.response_ms.push(Some(response_ms));
        if too_slow {
            self.too_slow += 1;
        }
    }

    /// Aucune action reçue avant le délai réseau.
    pub fn record_missing(&mut self) {
        self.compute_ms.push(None);
        self.response_ms.push(None);
        self.missing += 1;
    }

    pub fn record_rtt(&mut self, rtt_ms: f64) {
        self.rtt_ms.push(rtt_ms);
    }

    /// Bilan du match pour ce participant (colonne `timing`).
    pub fn summary(&self, compute_budget_ms: f64) -> TimingSummary {
        let mut computes: Vec<f64> = self.compute_ms.iter().flatten().copied().collect();
        computes.sort_by(f64::total_cmp);
        // Le plus petit aller-retour observé : le plancher du réseau. Les
        // plus grands incluent de la gigue ponctuelle.
        let rtt_ms = self.rtt_ms.iter().copied().reduce(f64::min);

        let median_unexplained_ms = rtt_ms.and_then(|rtt| {
            let mut unexplained: Vec<f64> = self
                .compute_ms
                .iter()
                .zip(&self.response_ms)
                .filter_map(|(compute, response)| Some((response.as_ref()? - compute.as_ref()? - rtt).max(0.0)))
                .collect();
            unexplained.sort_by(f64::total_cmp);
            percentile(&unexplained, 0.5)
        });

        TimingSummary {
            compute_budget_ms,
            mean_compute_ms: (!computes.is_empty()).then(|| round(computes.iter().sum::<f64>() / computes.len() as f64)),
            p95_compute_ms: percentile(&computes, 0.95).map(round),
            max_compute_ms: computes.last().copied().map(round),
            too_slow: self.too_slow,
            missing: self.missing,
            rtt_ms: rtt_ms.map(round),
            median_unexplained_ms: median_unexplained_ms.map(round),
            suspect: median_unexplained_ms.is_some_and(|ms| ms > SUSPECT_UNEXPLAINED_MS),
        }
    }

    /// Les temps pas par pas (colonne `step_timings`), pour la recherche.
    pub fn steps(&self) -> serde_json::Value {
        let rounded = |values: &[Option<f64>]| values.iter().map(|value| value.map(round)).collect::<Vec<_>>();
        serde_json::json!({
            "compute_ms": rounded(&self.compute_ms),
            "response_ms": rounded(&self.response_ms),
            "rtt_ms": self.rtt_ms.iter().copied().map(round).collect::<Vec<_>>(),
        })
    }
}

/// Bilan des temps d'un participant sur un match.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimingSummary {
    pub compute_budget_ms: f64,
    pub mean_compute_ms: Option<f64>,
    pub p95_compute_ms: Option<f64>,
    pub max_compute_ms: Option<f64>,
    /// Actions calculées en plus que le budget (remplacées par l'action neutre).
    pub too_slow: u32,
    /// Actions jamais reçues (remplacées par l'action neutre).
    pub missing: u32,
    /// Aller-retour réseau (le plus petit mesuré), `None` si aucun `Ping`
    /// n'a reçu de réponse.
    pub rtt_ms: Option<f64>,
    /// Temps de réponse que ni le réseau ni le calcul déclaré n'expliquent
    /// (médiane sur le match).
    pub median_unexplained_ms: Option<f64>,
    /// Temps de calcul probablement sous-déclarés (cf. `SUSPECT_UNEXPLAINED_MS`).
    pub suspect: bool,
}

/// Valeur au rang `q` (0..=1) d'une liste DÉJÀ triée (`None` si vide).
fn percentile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let index = ((sorted.len() - 1) as f64 * q).round() as usize;
    Some(sorted[index])
}

/// Au centième de milliseconde : bien assez précis, et beaucoup plus
/// compact une fois écrit en JSON.
fn round(ms: f64) -> f64 {
    (ms * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(steps: &[(f64, f64)], rtt: &[f64]) -> SeatTiming {
        let mut timing = SeatTiming::default();
        for &(compute, response) in steps {
            timing.record_action(Some(compute), response, false);
        }
        for &rtt in rtt {
            timing.record_rtt(rtt);
        }
        timing
    }

    #[test]
    fn an_honest_distant_client_is_not_suspect() {
        // Loin (150 ms d'aller-retour), calcul rapide : la réponse est
        // presque entièrement expliquée par le réseau.
        let summary = timing(&[(5.0, 157.0), (7.0, 160.0), (6.0, 158.0)], &[180.0, 151.0]).summary(30.0);

        assert_eq!(summary.rtt_ms, Some(151.0));
        assert_eq!(summary.mean_compute_ms, Some(6.0));
        assert_eq!(summary.median_unexplained_ms, Some(1.0));
        assert!(!summary.suspect);
    }

    #[test]
    fn an_under_declared_compute_time_is_suspect() {
        // Déclare 1 ms, mais met 300 ms de plus que l'aller-retour à répondre.
        let summary = timing(&[(1.0, 321.0), (1.0, 330.0), (1.0, 318.0)], &[20.0]).summary(30.0);

        assert_eq!(summary.median_unexplained_ms, Some(300.0));
        assert!(summary.suspect);
    }

    #[test]
    fn without_a_measured_round_trip_nothing_is_concluded() {
        let summary = timing(&[(1.0, 500.0)], &[]).summary(30.0);

        assert_eq!(summary.rtt_ms, None);
        assert_eq!(summary.median_unexplained_ms, None);
        assert!(!summary.suspect);
    }

    #[test]
    fn missing_and_too_slow_actions_are_counted() {
        let mut timing = SeatTiming::default();
        timing.record_action(Some(45.0), 50.0, true);
        timing.record_missing();
        timing.record_action(Some(10.0), 12.0, false);

        let summary = timing.summary(30.0);

        assert_eq!((summary.too_slow, summary.missing), (1, 1));
        assert_eq!(summary.max_compute_ms, Some(45.0));
        assert_eq!(timing.steps()["compute_ms"], serde_json::json!([45.0, null, 10.0]));
    }
}
