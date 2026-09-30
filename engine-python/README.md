# frondori-engine

Les environnements de recherche de Frondori, au format
[PettingZoo](https://pettingzoo.farama.org/) : même contrat pour tous,
qu'ils soient écrits en Rust ou en Python. Utilisables directement en local,
**sans serveur, sans réseau, sans token**, et compatibles avec l'écosystème
qui parle PettingZoo (RLlib, TorchRL, CleanRL, SuperSuit...).

Ce paquet n'est **pas** le SDK de compétition (`frondori-sdk`, séparé) : il
ne joue aucun match classé. Il sert à itérer vite sur une politique ou une
idée d'environnement.

## Installation

```bash
pip install frondori-engine
```

Wheel précompilée : aucun compilateur Rust requis.

## Démarrage rapide

```python
import frondori_engine

env = frondori_engine.make("football-v0")
observations, infos = env.reset(seed=42)

while env.agents:
    actions = {agent: env.action_space(agent).sample() for agent in env.agents}
    observations, rewards, terminations, truncations, infos = env.step(actions)
```

Le même code marche pour tous les environnements : remplacer
`"football-v0"` par `"kitchen-v0"` suffit. Voir
[`examples/random_agent.py`](examples/random_agent.py).

## Environnements disponibles

`frondori_engine.registered_ids()` liste les environnements installés.

| Id | Agents | Type | Écrit en |
|---|---|---|---|
| `football-v0` | `team_0`, `team_1` (une équipe par agent) | compétitif | Rust (moteur physique rapier2d) |
| `kitchen-v0` | `chef_0`, `chef_1` | coopératif, récompense partagée | Python |

Le détail des observations, actions et récompenses de chaque environnement
est dans la docstring de son module (`frondori_engine/envs/`), et leur forme
exacte dans `env.observation_space(agent)` / `env.action_space(agent)`.

Les paramètres d'un environnement se passent à `make` :

```python
frondori_engine.make("football-v0", players_per_team=5, max_score=3)
frondori_engine.make("kitchen-v0", onions_needed=3, cook_time=20, layout=(...))
```

## Versions

Chaque identifiant porte une version (`nom-vN`). Changer les règles d'un
environnement, c'est publier `nom-v(N+1)`, jamais modifier `nom-vN` : un
résultat obtenu sur `football-v0` reste comparable dans le temps.

## Déterminisme

Même seed + mêmes actions = même épisode, pas pour pas (vérifié par les tests
officiels PettingZoo, sur chaque environnement).

## Rendu

`make(..., render_mode="scene")` puis `env.render()` renvoie une scène
générique — rectangles, cercles, texte — sérialisable en JSON. Tout
environnement se dessine ainsi avec le même afficheur, sans code spécifique
au jeu. Le format est décrit dans `frondori_engine/scene.py`.

## Ajouter un environnement

1. Un module dans `python/frondori_engine/envs/` qui implémente
   `pettingzoo.ParallelEnv` (spaces Gymnasium, `render_mode="scene"`).
2. Une ligne `register("mon_env-v0", MonEnv)` dans
   `python/frondori_engine/__init__.py`.

Les tests de contrat (`tests/test_contract.py`) s'appliquent alors
automatiquement au nouvel environnement : API PettingZoo, déterminisme,
observations conformes à leur space, rendu sérialisable.

Un environnement qui a besoin de performances natives s'écrit en Rust, dans
le même esprit que le football (`src/lib.rs` expose le moteur, un module
Python l'enveloppe en `ParallelEnv`).

## Développer ce paquet

```bash
python -m venv .venv && source .venv/bin/activate
pip install maturin
maturin develop            # build du module natif + installation du paquet
pip install -e ".[dev]"
python -m pytest
```

Ce crate fait partie du workspace Cargo `frondori-server` (dépendance locale
directe sur `engine`) mais se publie indépendamment sur PyPI, via
`maturin publish`.
