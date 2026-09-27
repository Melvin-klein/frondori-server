"""Équivalent Python de `engine/examples/random_agent.rs` : fait tourner un
match complet entre deux équipes jouant des actions aléatoires, entièrement
en local (aucun réseau, aucun serveur).

Lancer avec : python examples/random_agent.py
"""

import random

from frondori_engine import Action, Actions, Engine, EngineConfig


def random_actions(n_players: int) -> Actions:
    return Actions(players=[
        Action(move_dir=(random.uniform(-1, 1), random.uniform(-1, 1)))
        for _ in range(n_players)
    ])


def main() -> None:
    # Seed fixe : relancer ce script produit exactement le même match.
    seed = 42
    config = EngineConfig()
    players_per_team = config.players_per_team

    engine = Engine(config, seed)
    engine.reset()

    tick = 0
    while True:
        actions = (random_actions(players_per_team), random_actions(players_per_team))
        result = engine.step(actions)
        print(f"tick {tick}: score A={result.observations[0].score} reward A={result.rewards[0].total()}")

        if result.done:
            print(f"Match terminé après {tick} ticks : score final {result.observations[0].score}")
            break

        tick += 1


if __name__ == "__main__":
    main()
