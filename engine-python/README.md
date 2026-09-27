# frondori-engine

Environnement de simulation football 2D de la plateforme Frondori, natif
(Rust, via [PyO3](https://pyo3.rs)/[maturin](https://www.maturin.rs)),
utilisable directement depuis Python — API façon Gym (`reset()` / `step()`),
**sans serveur, sans réseau, sans token**.

Ce paquet n'est **pas** le SDK de compétition (`frondori-sdk`, séparé) : il
ne parle à aucun serveur et ne peut pas jouer un vrai match classé. Il sert à
itérer vite sur une politique en local, avant de la brancher sur `frondori-sdk`
pour l'exécuter en compétition réelle.

Ce paquet est une fine couche de traduction PyO3 autour du crate Rust
`engine` (dans le même dépôt, `frondori-server`) : toute la logique de
simulation (physique rapier2d, déterminisme, détection de but...) vit là-bas,
inchangée — ce paquet ne fait qu'exposer son API à Python.

## Installation

```bash
pip install frondori-engine
```

(Wheel précompilée — aucun compilateur Rust requis.)

## Démarrage rapide

```python
from frondori_engine import Action, Actions, Engine, EngineConfig

config = EngineConfig()  # 3v3 par défaut, cf. les valeurs par défaut ci-dessous
engine = Engine(config, seed=42)

obs_a, obs_b = engine.reset()

result = engine.step((
    Actions(players=[Action(move_dir=(1.0, 0.0)) for _ in obs_a.self_team]),
    Actions(players=[Action() for _ in obs_b.self_team]),  # Action() = NOOP
))

print(result.observations[0].score, result.rewards[0].total(), result.done)
```

Voir [`examples/random_agent.py`](examples/random_agent.py) pour un exemple
complet (équivalent Python de `engine/examples/random_agent.rs`).

## API

- `EngineConfig(...)` : tous les paramètres ont une valeur par défaut
  (mêmes valeurs que `engine::config::EngineConfig::default()` côté Rust) —
  `players_per_team=3`, `field_width=40.0`, `field_height=20.0`,
  `max_ticks=9000` (5 min à 30 Hz), `max_score=None`, etc.
- `Engine(config, seed)` : `reset()` renvoie `(Observation, Observation)` ;
  `step((actions_a, actions_b))` renvoie un `StepResult`
  (`.observations`, `.rewards`, `.done`, `.info`).
- `Action(move_dir=(0.0, 0.0), kick=None)` : une action pour un joueur.
  `kick` est soit `None` (pas de tir ce tick), soit un tuple `(dx, dy)` —
  direction du tir, puissance encodée dans sa norme.
- `Actions(players=[...])` : les actions de tous les joueurs d'UNE équipe.
- `Observation.self_team` / `.opponent_team` : listes de `PlayerObs(position, velocity)`,
  toujours du point de vue de l'équipe qui reçoit l'observation (on attaque
  toujours vers `x = +1`, quel que soit le côté réel du terrain).
- `engine.spectator_frame()` : état ABSOLU du match (jamais mirroré, avec
  l'équipe de chaque joueur) — pour visualiser localement, indépendamment de
  ce que `step()` renvoie aux deux équipes.

## Déterminisme

Même seed + mêmes actions à chaque tick = même résultat, tick pour tick.
Utile pour rejouer/déboguer un épisode exact, ou paralléliser un
entraînement avec des runs reproductibles.

## Développer / builder ce paquet

```bash
python -m venv .venv && source .venv/bin/activate
pip install maturin
maturin develop   # build + installe le module dans le venv courant
```

Ce crate fait partie du workspace Cargo `frondori-server` (dépendance locale
directe sur `engine`, `path = "../engine"`) mais se publie et se release
indépendamment, via `maturin publish` (wheels PyPI), depuis ce même dépôt.
