"""`kitchen-v0` : mini-cuisine coopérative à deux agents, façon Overcooked.

Volontairement à l'opposé du football, pour vérifier que le contrat commun
ne cache aucune hypothèse propre au football :
- coopératif : aucun adversaire, les deux agents reçoivent exactement la
  même récompense ;
- discret : une grille, des actions `Discrete`, aucune physique ;
- écrit en Python pur, sans une ligne de Rust.

Règles : deux chefs (`chef_0`, `chef_1`) dans une cuisine en grille. Déposer
`onions_needed` oignons dans une marmite lance la cuisson, qui dure
`cook_time` pas. Une fois la soupe prête, il faut la récupérer avec une
assiette puis la porter au passe : +1 pour les deux chefs par soupe servie.
Les plans de travail permettent de poser un objet — et donc de se le passer.

Plan (`layout`), une chaîne par ligne :
    X  plan de travail        O  réserve d'oignons    D  réserve d'assiettes
    P  marmite                S  passe (service)      ' ' sol
    1, 2  sol, position de départ de chef_0 / chef_1

Actions (Discrete(6)) : 0 rester, 1 haut, 2 bas, 3 gauche, 4 droite,
5 interagir avec la case en face. Se déplacer vers une case non praticable
fait seulement se tourner dans cette direction.

Observation (Dict), toujours du point de vue de l'agent qui la reçoit :
- `self`, `partner` : `[ligne, colonne, orientation, objet tenu]` ;
- `pots` : une ligne `[oignons, cuisson restante]` par marmite ;
- `counters` : l'objet posé sur chaque plan de travail ;
- `time_remaining` : pas restants avant la fin de l'épisode.
Objets : 0 rien, 1 oignon, 2 assiette, 3 soupe. Orientations : 0 haut,
1 bas, 2 gauche, 3 droite. Marmites et plans de travail sont listés dans
l'ordre de lecture du plan (ligne par ligne, de gauche à droite).
"""

from __future__ import annotations

import gymnasium
import numpy as np
from gymnasium import spaces
from pettingzoo import ParallelEnv

from frondori_engine import scene as sc

DEFAULT_LAYOUT = (
    "XXPXX",
    "O1 2S",
    "X   X",
    "XDXXX",
)

NOTHING, ONION, DISH, SOUP = 0, 1, 2, 3
UP, DOWN, LEFT, RIGHT = 0, 1, 2, 3
STAY, MOVE_UP, MOVE_DOWN, MOVE_LEFT, MOVE_RIGHT, INTERACT = range(6)

_MOVES = {MOVE_UP: UP, MOVE_DOWN: DOWN, MOVE_LEFT: LEFT, MOVE_RIGHT: RIGHT}
_DELTAS = {UP: (-1, 0), DOWN: (1, 0), LEFT: (0, -1), RIGHT: (0, 1)}
_FLOOR = {" ", "1", "2"}

_TILE_COLORS = {"X": "#cbd5e1", "O": "#fde68a", "D": "#bae6fd", "P": "#475569", "S": "#86efac"}
_ITEM_COLORS = {ONION: "#eab308", DISH: "#64748b", SOUP: "#f97316"}
_CHEF_COLORS = ("#3b82f6", "#ef4444")


class KitchenEnv(ParallelEnv):
    # `render_fps` : cadence d'un match en compétition. 5 pas par seconde,
    # soit 200 ms de réflexion par décision et 40 s pour un épisode complet.
    metadata = {"name": "kitchen_v0", "render_modes": ["scene"], "render_fps": 5, "is_parallelizable": True}

    def __init__(
        self,
        render_mode: str | None = None,
        layout: tuple[str, ...] = DEFAULT_LAYOUT,
        onions_needed: int = 2,
        cook_time: int = 5,
        max_steps: int = 200,
    ):
        if render_mode is not None and render_mode not in self.metadata["render_modes"]:
            raise ValueError(f"render_mode {render_mode!r} non supporté ; disponibles : {self.metadata['render_modes']}")
        self.render_mode = render_mode
        self._layout = tuple(layout)
        self._rows, self._cols = len(self._layout), len(self._layout[0])
        if any(len(row) != self._cols for row in self._layout):
            raise ValueError("plan invalide : toutes les lignes doivent avoir la même longueur")

        cells = [(r, c) for r in range(self._rows) for c in range(self._cols)]
        self._spawns = []
        for marker in ("1", "2"):
            found = [cell for cell in cells if self._tile(cell) == marker]
            if len(found) != 1:
                raise ValueError(f"plan invalide : il faut exactement une case {marker!r}")
            self._spawns.append(found[0])
        self._pot_cells = [cell for cell in cells if self._tile(cell) == "P"]
        self._counter_cells = [cell for cell in cells if self._tile(cell) == "X"]

        self._onions_needed = onions_needed
        self._cook_time = cook_time
        self._max_steps = max_steps

        self.possible_agents = ["chef_0", "chef_1"]
        self.agents = []
        self._observation_spaces = {agent: self._make_observation_space() for agent in self.possible_agents}
        self._action_spaces = {agent: spaces.Discrete(6) for agent in self.possible_agents}

    def _make_observation_space(self) -> spaces.Dict:
        chef = [self._rows, self._cols, 4, 4]
        return spaces.Dict({
            "self": spaces.MultiDiscrete(chef),
            "partner": spaces.MultiDiscrete(chef),
            "pots": spaces.MultiDiscrete(
                np.tile([self._onions_needed + 1, self._cook_time + 1], (len(self._pot_cells), 1))
            ),
            "counters": spaces.MultiDiscrete([4] * len(self._counter_cells)),
            "time_remaining": spaces.Discrete(self._max_steps + 1),
        })

    def observation_space(self, agent: str) -> spaces.Space:
        return self._observation_spaces[agent]

    def action_space(self, agent: str) -> spaces.Space:
        return self._action_spaces[agent]

    def reset(self, seed: int | None = None, options: dict | None = None):
        # Aucun aléa dans cet environnement : `seed` est accepté pour
        # respecter le contrat, mais n'a aucun effet.
        self._positions = list(self._spawns)
        self._facing = [UP, UP]
        self._holding = [NOTHING, NOTHING]
        self._counters = {cell: NOTHING for cell in self._counter_cells}
        self._pots = {cell: [0, 0] for cell in self._pot_cells}
        self._steps = 0
        self._served = 0
        self.agents = list(self.possible_agents)
        return self._observations(), {agent: {} for agent in self.agents}

    def step(self, actions: dict):
        chosen = [int(actions[agent]) for agent in self.possible_agents]
        for action in chosen:
            if not 0 <= action < 6:
                raise ValueError(f"action {action} invalide : attendu un entier entre 0 et 5")

        self._move(chosen)
        # Interactions dans l'ordre des agents : si les deux visent la même
        # marmite au même pas, chef_0 passe en premier (déterministe).
        reward = sum(self._interact(i) for i, action in enumerate(chosen) if action == INTERACT)
        for pot in self._pots.values():
            if pot[0] == self._onions_needed and pot[1] > 0:
                pot[1] -= 1
        self._steps += 1

        truncated = self._steps >= self._max_steps
        rewards = {agent: float(reward) for agent in self.agents}
        terminations = {agent: False for agent in self.agents}
        truncations = {agent: truncated for agent in self.agents}
        infos = {agent: {"served": self._served} for agent in self.agents}
        observations = self._observations()
        if truncated:
            self.agents = []
        return observations, rewards, terminations, truncations, infos

    def _move(self, chosen: list[int]) -> None:
        start = list(self._positions)
        targets = []
        for i, action in enumerate(chosen):
            target = start[i]
            if action in _MOVES:
                self._facing[i] = _MOVES[action]
                ahead = self._ahead(i)
                if self._tile(ahead) in _FLOOR:
                    target = ahead
            targets.append(target)
        # Déplacements simultanés : on ne peut ni entrer sur la case que
        # l'autre occupait au début du pas (ce qui interdit aussi de
        # s'échanger), ni viser la même case que lui.
        for i in range(2):
            other = 1 - i
            if targets[i] not in (start[other], targets[other]):
                self._positions[i] = targets[i]

    def _interact(self, i: int) -> float:
        cell = self._ahead(i)
        tile = self._tile(cell)
        holding = self._holding[i]

        if tile == "O" and holding == NOTHING:
            self._holding[i] = ONION
        elif tile == "D" and holding == NOTHING:
            self._holding[i] = DISH
        elif tile == "P":
            pot = self._pots[cell]
            if holding == ONION and pot[0] < self._onions_needed:
                pot[0] += 1
                self._holding[i] = NOTHING
                if pot[0] == self._onions_needed:
                    pot[1] = self._cook_time
            elif holding == DISH and pot[0] == self._onions_needed and pot[1] == 0:
                pot[0] = 0
                self._holding[i] = SOUP
        elif tile == "S" and holding == SOUP:
            self._holding[i] = NOTHING
            self._served += 1
            return 1.0
        elif tile == "X" and (holding == NOTHING) != (self._counters[cell] == NOTHING):
            # Poser (mains pleines, plan vide) ou ramasser (mains vides, plan
            # occupé) — jamais d'échange entre deux objets.
            self._holding[i], self._counters[cell] = self._counters[cell], holding
        return 0.0

    def _ahead(self, i: int) -> tuple[int, int]:
        (row, col), (d_row, d_col) = self._positions[i], _DELTAS[self._facing[i]]
        return row + d_row, col + d_col

    def _tile(self, cell: tuple[int, int]) -> str | None:
        row, col = cell
        if 0 <= row < self._rows and 0 <= col < self._cols:
            return self._layout[row][col]
        return None

    def _chef_state(self, i: int) -> np.ndarray:
        return np.array([*self._positions[i], self._facing[i], self._holding[i]], dtype=np.int64)

    def _observations(self) -> dict:
        pots = np.array([self._pots[cell] for cell in self._pot_cells], dtype=np.int64).reshape(-1, 2)
        counters = np.array([self._counters[cell] for cell in self._counter_cells], dtype=np.int64)
        time_remaining = self._max_steps - self._steps
        return {
            agent: {
                "self": self._chef_state(i),
                "partner": self._chef_state(1 - i),
                "pots": pots.copy(),
                "counters": counters.copy(),
                "time_remaining": time_remaining,
            }
            for i, agent in enumerate(self.possible_agents)
        }

    def render(self):
        if self.render_mode is None:
            gymnasium.logger.warn("render() appelé sans render_mode ; passer render_mode='scene' à make().")
            return None

        # Une bande d'une case en haut pour le texte (soupes servies, temps
        # restant) ; la grille est dessinée juste en dessous.
        top = 1
        shapes = [sc.text(
            self._cols / 2, 0.5,
            f"Servies : {self._served}   Temps : {self._max_steps - self._steps}",
            size=0.4,
        )]
        for row in range(self._rows):
            for col in range(self._cols):
                tile = self._layout[row][col]
                shapes.append(sc.rect(col, row + top, 1, 1, _TILE_COLORS.get(tile, "#f8fafc")))
        for (row, col), item in self._counters.items():
            if item != NOTHING:
                shapes.append(sc.circle(col + 0.5, row + top + 0.5, 0.2, _ITEM_COLORS[item]))
        for (row, col), (onions, cooking) in self._pots.items():
            if onions == self._onions_needed and cooking == 0:
                label = "prête"
            elif onions == self._onions_needed:
                label = str(cooking)
            else:
                label = f"{onions}/{self._onions_needed}"
            shapes.append(sc.text(col + 0.5, row + top + 0.5, label, fill="#ffffff", size=0.3))
        for i in range(2):
            row, col = self._positions[i]
            d_row, d_col = _DELTAS[self._facing[i]]
            center_x, center_y = col + 0.5, row + top + 0.5
            shapes.append(sc.circle(center_x, center_y, 0.35, _CHEF_COLORS[i]))
            shapes.append(sc.circle(center_x + 0.3 * d_col, center_y + 0.3 * d_row, 0.08, "#0f172a"))
            if self._holding[i] != NOTHING:
                shapes.append(sc.circle(center_x, center_y, 0.15, _ITEM_COLORS[self._holding[i]]))
        return sc.scene(self._cols, self._rows + top, shapes, background="#f8fafc")
