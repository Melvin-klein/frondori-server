"""Environnements de recherche Frondori, au format PettingZoo.

    import frondori_engine

    env = frondori_engine.make("football-v0")
    observations, infos = env.reset(seed=42)
    while env.agents:
        actions = {agent: env.action_space(agent).sample() for agent in env.agents}
        observations, rewards, terminations, truncations, infos = env.step(actions)

Tous les environnements suivent le même contrat (`pettingzoo.ParallelEnv`,
spaces Gymnasium, rendu `render_mode="scene"`), qu'ils soient écrits en Rust
(le football, via le module natif `_football`) ou en Python pur (la cuisine).
Ajouter un environnement : un module dans `envs/`, une ligne `register(...)`
ci-dessous.
"""

from frondori_engine.envs.football import FootballEnv
from frondori_engine.envs.kitchen import KitchenEnv
from frondori_engine.registry import EnvSpec, make, register, registered_ids

register("football-v0", FootballEnv)
register("kitchen-v0", KitchenEnv)

__all__ = ["EnvSpec", "FootballEnv", "KitchenEnv", "make", "register", "registered_ids"]
