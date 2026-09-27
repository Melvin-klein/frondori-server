//! Bindings Python (PyO3) autour du crate `engine`, pur Rust.
//!
//! Ce crate ne contient AUCUNE logique de simulation : c'est une couche de
//! traduction, dans les deux sens, entre les types de `engine` et des types
//! Python. `engine` lui-même reste totalement ignorant de Python — c'est
//! volontaire (cf. le commentaire en tête de `Cargo.toml`) : `server`/
//! `protocol` n'ont aucune raison de linker contre `libpython`.
//!
//! Convention de nommage : chaque type exposé porte le même nom que son
//! équivalent `engine::` (`Observation`, `Action`...), pour que la doc et le
//! mental mapping restent triviaux d'un langage à l'autre. Comme les règles
//! d'orphelinat de Rust interdisent d'implémenter `#[pyclass]` directement
//! sur un type d'un autre crate, chaque type ici est un NOUVEAU struct qui
//! enveloppe (ou reconstruit) son équivalent `engine::`, avec des
//! conversions `From`/`into()` explicites.

use pyo3::prelude::*;

use engine::config::EngineConfig as CoreEngineConfig;
use engine::types::{
    Action as CoreAction, Actions as CoreActions, BallObs as CoreBallObs,
    Info as CoreInfo, Observation as CoreObservation, PlayerObs as CorePlayerObs,
    Reward as CoreReward, SpectatorFrame as CoreSpectatorFrame,
    SpectatorPlayer as CoreSpectatorPlayer, StepResult as CoreStepResult,
};
use engine::Engine as CoreEngine;

// ---------------------------------------------------------------------
// EngineConfig — le seul type qu'un utilisateur construit avec beaucoup de
// paramètres ; les valeurs par défaut ci-dessous sont recopiées de
// `engine::config::EngineConfig::default()` (aucune raison qu'elles divergent
// silencieusement, mais rien ne les synchronise automatiquement : à
// mettre à jour à la main si le Rust change un jour).
// ---------------------------------------------------------------------

#[pyclass(get_all, set_all, from_py_object)]
#[derive(Clone)]
pub struct EngineConfig {
    pub players_per_team: usize,
    pub field_width: f32,
    pub field_height: f32,
    pub player_radius: f32,
    pub ball_radius: f32,
    pub goal_width: f32,
    pub dt: f32,
    pub max_ticks: u32,
    pub max_score: Option<u32>,
    pub player_max_speed: f32,
    pub ball_linear_damping: f32,
    pub ball_restitution: f32,
    pub kick_range: f32,
    pub kick_max_speed: f32,
    pub formation_jitter: f32,
}

#[pymethods]
impl EngineConfig {
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (
        players_per_team=3,
        field_width=40.0,
        field_height=20.0,
        player_radius=0.5,
        ball_radius=0.2,
        goal_width=6.0,
        dt=1.0/30.0,
        max_ticks=9000,
        max_score=None,
        player_max_speed=6.0,
        ball_linear_damping=0.6,
        ball_restitution=0.6,
        kick_range=0.4,
        kick_max_speed=12.0,
        formation_jitter=0.5,
    ))]
    fn new(
        players_per_team: usize,
        field_width: f32,
        field_height: f32,
        player_radius: f32,
        ball_radius: f32,
        goal_width: f32,
        dt: f32,
        max_ticks: u32,
        max_score: Option<u32>,
        player_max_speed: f32,
        ball_linear_damping: f32,
        ball_restitution: f32,
        kick_range: f32,
        kick_max_speed: f32,
        formation_jitter: f32,
    ) -> Self {
        Self {
            players_per_team,
            field_width,
            field_height,
            player_radius,
            ball_radius,
            goal_width,
            dt,
            max_ticks,
            max_score,
            player_max_speed,
            ball_linear_damping,
            ball_restitution,
            kick_range,
            kick_max_speed,
            formation_jitter,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "EngineConfig(players_per_team={}, field_width={}, field_height={}, max_ticks={})",
            self.players_per_team, self.field_width, self.field_height, self.max_ticks
        )
    }
}

impl From<&EngineConfig> for CoreEngineConfig {
    fn from(c: &EngineConfig) -> Self {
        CoreEngineConfig {
            players_per_team: c.players_per_team,
            field_width: c.field_width,
            field_height: c.field_height,
            player_radius: c.player_radius,
            ball_radius: c.ball_radius,
            goal_width: c.goal_width,
            dt: c.dt,
            max_ticks: c.max_ticks,
            max_score: c.max_score,
            player_max_speed: c.player_max_speed,
            ball_linear_damping: c.ball_linear_damping,
            ball_restitution: c.ball_restitution,
            kick_range: c.kick_range,
            kick_max_speed: c.kick_max_speed,
            formation_jitter: c.formation_jitter,
        }
    }
}

// ---------------------------------------------------------------------
// Action / Actions — entrée de step()
// ---------------------------------------------------------------------

#[pyclass(get_all, from_py_object)]
#[derive(Clone, Copy)]
pub struct Action {
    pub move_dir: (f32, f32),
    pub kick: Option<(f32, f32)>,
}

#[pymethods]
impl Action {
    #[new]
    #[pyo3(signature = (move_dir=(0.0, 0.0), kick=None))]
    fn new(move_dir: (f32, f32), kick: Option<(f32, f32)>) -> Self {
        Self { move_dir, kick }
    }

    /// Équivalent de `Action::NOOP` côté Rust : aucun déplacement, aucun tir.
    #[staticmethod]
    fn noop() -> Self {
        CoreAction::NOOP.into()
    }

    fn __repr__(&self) -> String {
        format!("Action(move_dir={:?}, kick={:?})", self.move_dir, self.kick)
    }
}

impl From<CoreAction> for Action {
    fn from(a: CoreAction) -> Self {
        Self { move_dir: a.move_dir, kick: a.kick }
    }
}

impl From<Action> for CoreAction {
    fn from(a: Action) -> Self {
        CoreAction { move_dir: a.move_dir, kick: a.kick }
    }
}

#[pyclass(get_all, from_py_object)]
#[derive(Clone)]
pub struct Actions {
    pub players: Vec<Action>,
}

#[pymethods]
impl Actions {
    #[new]
    fn new(players: Vec<Action>) -> Self {
        Self { players }
    }
}

impl From<Actions> for CoreActions {
    fn from(a: Actions) -> Self {
        CoreActions {
            players: a.players.into_iter().map(Into::into).collect(),
        }
    }
}

// ---------------------------------------------------------------------
// Observation / PlayerObs / BallObs — sortie de reset()/step(), lecture seule
// ---------------------------------------------------------------------

#[pyclass(get_all, from_py_object)]
#[derive(Clone, Copy)]
pub struct PlayerObs {
    pub position: (f32, f32),
    pub velocity: (f32, f32),
}

impl From<CorePlayerObs> for PlayerObs {
    fn from(p: CorePlayerObs) -> Self {
        Self { position: p.position, velocity: p.velocity }
    }
}

#[pyclass(get_all, from_py_object)]
#[derive(Clone, Copy)]
pub struct BallObs {
    pub position: (f32, f32),
    pub velocity: (f32, f32),
}

impl From<CoreBallObs> for BallObs {
    fn from(b: CoreBallObs) -> Self {
        Self { position: b.position, velocity: b.velocity }
    }
}

#[pyclass(get_all, from_py_object)]
#[derive(Clone)]
pub struct Observation {
    pub self_team: Vec<PlayerObs>,
    pub opponent_team: Vec<PlayerObs>,
    pub ball: BallObs,
    pub score: (u32, u32),
    pub ticks_remaining: u32,
}

impl From<CoreObservation> for Observation {
    fn from(o: CoreObservation) -> Self {
        Self {
            self_team: o.self_team.into_iter().map(Into::into).collect(),
            opponent_team: o.opponent_team.into_iter().map(Into::into).collect(),
            ball: o.ball.into(),
            score: o.score,
            ticks_remaining: o.ticks_remaining,
        }
    }
}

// ---------------------------------------------------------------------
// Reward / Info / StepResult — sortie de step(), lecture seule
// ---------------------------------------------------------------------

#[pyclass(get_all, from_py_object)]
#[derive(Clone, Copy)]
pub struct Reward {
    pub goal_scored: f32,
    pub goal_conceded: f32,
}

#[pymethods]
impl Reward {
    fn total(&self) -> f32 {
        self.goal_scored + self.goal_conceded
    }
}

impl From<CoreReward> for Reward {
    fn from(r: CoreReward) -> Self {
        Self { goal_scored: r.goal_scored, goal_conceded: r.goal_conceded }
    }
}

#[pyclass(get_all, from_py_object)]
#[derive(Clone)]
pub struct Info {
    pub tick: u32,
}

impl From<CoreInfo> for Info {
    fn from(i: CoreInfo) -> Self {
        Self { tick: i.tick }
    }
}

#[pyclass(get_all, from_py_object)]
#[derive(Clone)]
pub struct StepResult {
    /// `(observation équipe A, observation équipe B)`.
    pub observations: (Observation, Observation),
    /// `(reward équipe A, reward équipe B)`.
    pub rewards: (Reward, Reward),
    pub done: bool,
    pub info: Info,
}

impl From<CoreStepResult> for StepResult {
    fn from(r: CoreStepResult) -> Self {
        let [obs_a, obs_b] = r.observations;
        let [rew_a, rew_b] = r.rewards;
        Self {
            observations: (obs_a.into(), obs_b.into()),
            rewards: (rew_a.into(), rew_b.into()),
            done: r.done,
            info: r.info.into(),
        }
    }
}

// ---------------------------------------------------------------------
// SpectatorPlayer / SpectatorFrame — état absolu, pour visualiser localement
// (même donnée que celle diffusée par `/spectate` côté serveur réseau).
// ---------------------------------------------------------------------

#[pyclass(get_all, from_py_object)]
#[derive(Clone, Copy)]
pub struct SpectatorPlayer {
    pub team: u8,
    pub position: (f32, f32),
    pub velocity: (f32, f32),
}

impl From<CoreSpectatorPlayer> for SpectatorPlayer {
    fn from(p: CoreSpectatorPlayer) -> Self {
        Self { team: p.team, position: p.position, velocity: p.velocity }
    }
}

#[pyclass(get_all, from_py_object)]
#[derive(Clone)]
pub struct SpectatorFrame {
    pub tick: u32,
    pub players: Vec<SpectatorPlayer>,
    pub ball: BallObs,
    pub score: (u32, u32),
}

impl From<CoreSpectatorFrame> for SpectatorFrame {
    fn from(f: CoreSpectatorFrame) -> Self {
        Self {
            tick: f.tick,
            players: f.players.into_iter().map(Into::into).collect(),
            ball: f.ball.into(),
            score: f.score,
        }
    }
}

// ---------------------------------------------------------------------
// Engine — le point d'entrée, API façon Gym : new(config, seed), reset(),
// step(actions). Mêmes noms/signatures que côté Rust, cf.
// `engine/examples/random_agent.rs`.
// ---------------------------------------------------------------------

#[pyclass]
pub struct Engine {
    inner: CoreEngine,
}

#[pymethods]
impl Engine {
    #[new]
    fn new(config: &EngineConfig, seed: u64) -> Self {
        Self { inner: CoreEngine::new(config.into(), seed) }
    }

    /// Remet le match à zéro (nouveau coup d'envoi) et renvoie l'observation
    /// initiale des deux équipes : `(observation_a, observation_b)`.
    fn reset(&mut self) -> (Observation, Observation) {
        let [a, b] = self.inner.reset();
        (a.into(), b.into())
    }

    /// Un pas de simulation. `actions` : `(actions_équipe_a, actions_équipe_b)`.
    fn step(&mut self, actions: (Actions, Actions)) -> StepResult {
        let (a, b) = actions;
        self.inner.step([a.into(), b.into()]).into()
    }

    /// État absolu du match au tick courant (jamais mirroré) — pour
    /// visualiser localement, indépendamment de ce que `step()` renvoie aux
    /// deux équipes.
    fn spectator_frame(&self) -> SpectatorFrame {
        self.inner.spectator_frame().into()
    }
}

// ---------------------------------------------------------------------
// Module Python (`import frondori_engine`)
// ---------------------------------------------------------------------

#[pymodule]
fn frondori_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<EngineConfig>()?;
    m.add_class::<Action>()?;
    m.add_class::<Actions>()?;
    m.add_class::<PlayerObs>()?;
    m.add_class::<BallObs>()?;
    m.add_class::<Observation>()?;
    m.add_class::<Reward>()?;
    m.add_class::<Info>()?;
    m.add_class::<StepResult>()?;
    m.add_class::<SpectatorPlayer>()?;
    m.add_class::<SpectatorFrame>()?;
    m.add_class::<Engine>()?;
    Ok(())
}
