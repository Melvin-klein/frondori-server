"""`football-v0` : le moteur physique Rust (`engine`) exposé comme
environnement PettingZoo parallèle.

Deux agents, `team_0` et `team_1`, contrôlent chacun une ÉQUIPE entière
(même convention qu'en compétition : un agent = une équipe). Chaque agent
reçoit sa propre observation, toujours de son point de vue : il attaque vers
x = +1, quel que soit son côté réel du terrain (le miroir est fait par le
moteur). Même chose pour les actions : +x veut toujours dire "vers le but
adverse".

Observation (Dict) :
- `self_team`, `opponent_team` : (N, 4) — `[pos_x, pos_y, vel_x, vel_y]`
  par joueur, positions normalisées dans [-1, 1] ;
- `ball` : (4,) — même format ;
- `score` : (2,) — `(buts marqués, buts encaissés)` ;
- `ticks_remaining` : (1,).

Action (Box, N x 5) : une ligne par joueur, `[move_x, move_y, kick_x,
kick_y, kick_flag]`. `kick_flag > 0.5` déclenche un tir dans la direction
`(kick_x, kick_y)`, la puissance étant la norme de ce vecteur.

Récompense : +1 au tick où l'équipe marque, -1 au tick où elle encaisse.

Infos (par équipe, cumulées depuis le début du match) : `score` (buts
marqués — clé conventionnelle, que le site affiche comme score du match) et
`possession` — la part des pas où un joueur de l'équipe
était le plus proche du ballon. Mesure approximative : le moteur n'a aucune
notion de contact ou de possession.
"""

from __future__ import annotations

import gymnasium
import numpy as np
from gymnasium import spaces
from pettingzoo import ParallelEnv

from frondori_engine import _football
from frondori_engine import scene as sc

_TEAM_COLORS = ("#3b82f6", "#ef4444")


class FootballEnv(ParallelEnv):
    metadata = {
        "name": "football_v0",
        "title": "Football 2D",
        "description": "Two teams face off on a 2D pitch: each agent controls a whole team.",
        # Duel : classement ELO, le vainqueur étant l'équipe au meilleur retour.
        "ranking": "elo",
        # Temps de calcul accordé à un agent pour chaque action, en
        # compétition (le réseau n'est pas compté : matchs en pas-à-pas).
        # Environ un pas de simulation (30 pas/s).
        "compute_budget_ms": 30,
        "render_modes": ["scene"],
        "is_parallelizable": True,
    }

    def __init__(self, render_mode: str | None = None, **config):
        """`config` : n'importe quel paramètre de `EngineConfig` côté Rust
        (`players_per_team`, `max_ticks`, `max_score`, `field_width`...)."""
        if render_mode is not None and render_mode not in self.metadata["render_modes"]:
            raise ValueError(f"render_mode {render_mode!r} non supporté ; disponibles : {self.metadata['render_modes']}")
        self.render_mode = render_mode
        self._config = _football.EngineConfig(**config)
        self._n = self._config.players_per_team
        # Un pas simule `dt` secondes : en compétition, le serveur joue donc
        # le match à 1/dt pas par seconde, pour qu'il se déroule en temps réel.
        # `dt` est un f32 côté Rust (1/30 n'y est pas exact) : arrondi pour
        # annoncer 30 pas/s et non 29.999998.
        self.metadata = {**self.metadata, "render_fps": round(1 / self._config.dt, 3)}

        self.possible_agents = ["team_0", "team_1"]
        self.agents = []
        self._observation_spaces = {agent: self._make_observation_space() for agent in self.possible_agents}
        self._action_spaces = {agent: self._make_action_space() for agent in self.possible_agents}

        self._rng = np.random.default_rng()
        self._engine = None

    def _make_observation_space(self) -> spaces.Dict:
        return spaces.Dict({
            "self_team": spaces.Box(-np.inf, np.inf, shape=(self._n, 4), dtype=np.float32),
            "opponent_team": spaces.Box(-np.inf, np.inf, shape=(self._n, 4), dtype=np.float32),
            "ball": spaces.Box(-np.inf, np.inf, shape=(4,), dtype=np.float32),
            "score": spaces.Box(0, np.inf, shape=(2,), dtype=np.float32),
            "ticks_remaining": spaces.Box(0, self._config.max_ticks, shape=(1,), dtype=np.float32),
        })

    def _make_action_space(self) -> spaces.Box:
        low = np.tile(np.array([-1, -1, -1, -1, 0], dtype=np.float32), (self._n, 1))
        high = np.ones((self._n, 5), dtype=np.float32)
        return spaces.Box(low, high, dtype=np.float32)

    def observation_space(self, agent: str) -> spaces.Space:
        return self._observation_spaces[agent]

    def action_space(self, agent: str) -> spaces.Space:
        return self._action_spaces[agent]

    def reset(self, seed: int | None = None, options: dict | None = None):
        if seed is not None:
            self._rng = np.random.default_rng(seed)
        # Le moteur Rust reçoit son seed à la construction : un nouvel épisode
        # = un nouveau moteur, dont le seed est tiré du générateur de
        # l'environnement — donc reproductible à `seed` égal.
        self._engine = _football.Engine(self._config, int(self._rng.integers(0, 2**63)))
        obs_0, obs_1 = self._engine.reset()
        self._possession_ticks = [0, 0]
        self.agents = list(self.possible_agents)
        observations = {"team_0": _observation(obs_0), "team_1": _observation(obs_1)}
        return observations, {agent: {} for agent in self.agents}

    def step(self, actions: dict):
        result = self._engine.step((
            _actions(actions["team_0"], self._n),
            _actions(actions["team_1"], self._n),
        ))
        obs_0, obs_1 = result.observations
        observations = {"team_0": _observation(obs_0), "team_1": _observation(obs_1)}
        rewards = {"team_0": result.rewards[0].total(), "team_1": result.rewards[1].total()}

        # Le moteur ne distingue pas les deux façons de finir ; Gymnasium si :
        # atteindre `max_score` est une vraie fin de partie (terminaison),
        # atteindre `max_ticks` est une coupure par limite de temps (troncature).
        max_score = self._config.max_score
        terminated = result.done and max_score is not None and max(obs_0.score) >= max_score
        truncated = result.done and not terminated

        terminations = {agent: terminated for agent in self.agents}
        truncations = {agent: truncated for agent in self.agents}
        self._possession_ticks[self._closest_team_to_ball()] += 1
        total = sum(self._possession_ticks)
        infos = {
            agent: {
                "score": int(obs.score[0]),
                "possession": round(self._possession_ticks[team] / total, 3),
            }
            for team, (agent, obs) in enumerate(zip(self.possible_agents, (obs_0, obs_1)))
            if agent in self.agents
        }
        if result.done:
            self.agents = []
        return observations, rewards, terminations, truncations, infos

    def _closest_team_to_ball(self) -> int:
        frame = self._engine.spectator_frame()
        ball_x, ball_y = frame.ball.position
        closest = min(
            frame.players,
            key=lambda player: (player.position[0] - ball_x) ** 2 + (player.position[1] - ball_y) ** 2,
        )
        return closest.team

    def render(self):
        if self.render_mode is None:
            gymnasium.logger.warn("render() appelé sans render_mode ; passer render_mode='scene' à make().")
            return None

        cfg = self._config
        width, height = cfg.field_width, cfg.field_height
        half_w, half_h = width / 2, height / 2
        frame = self._engine.spectator_frame()

        def to_scene(position):
            # `spectator_frame` : positions normalisées dans [-1, 1], centrées
            # sur le milieu du terrain, jamais mirrorées.
            return (position[0] + 1) * half_w, (position[1] + 1) * half_h

        goal_top = half_h - cfg.goal_width / 2
        shapes = [
            sc.rect(half_w - 0.05, 0, 0.1, height, "#86efac"),
            sc.rect(0, goal_top, 0.3, cfg.goal_width, "#f8fafc"),
            sc.rect(width - 0.3, goal_top, 0.3, cfg.goal_width, "#f8fafc"),
        ]
        for player in frame.players:
            x, y = to_scene(player.position)
            shapes.append(sc.circle(x, y, cfg.player_radius, _TEAM_COLORS[player.team]))
        ball_x, ball_y = to_scene(frame.ball.position)
        shapes.append(sc.circle(ball_x, ball_y, cfg.ball_radius, "#ffffff"))
        shapes.append(sc.text(half_w, 1.0, f"{frame.score[0]} - {frame.score[1]}", fill="#ffffff", size=1.2))
        return sc.scene(width, height, shapes, background="#15803d")


def _players(players) -> np.ndarray:
    return np.array([[*p.position, *p.velocity] for p in players], dtype=np.float32)


def _observation(obs) -> dict:
    return {
        "self_team": _players(obs.self_team),
        "opponent_team": _players(obs.opponent_team),
        "ball": np.array([*obs.ball.position, *obs.ball.velocity], dtype=np.float32),
        "score": np.array(obs.score, dtype=np.float32),
        "ticks_remaining": np.array([obs.ticks_remaining], dtype=np.float32),
    }


def _actions(action, n_players: int):
    rows = np.asarray(action, dtype=np.float32)
    if rows.shape != (n_players, 5):
        raise ValueError(f"action de forme {rows.shape}, attendu ({n_players}, 5) : une ligne par joueur")
    return _football.Actions([
        _football.Action(
            move_dir=(float(move_x), float(move_y)),
            kick=(float(kick_x), float(kick_y)) if kick_flag > 0.5 else None,
        )
        for move_x, move_y, kick_x, kick_y, kick_flag in rows
    ])
