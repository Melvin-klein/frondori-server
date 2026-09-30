"""Joue un épisode complet avec des actions aléatoires, dans n'importe quel
environnement du registre — même code pour tous, c'est tout l'intérêt du
contrat commun. Aucun serveur, aucun réseau.

Lancer avec :
    python examples/random_agent.py              # football-v0
    python examples/random_agent.py kitchen-v0
"""

import sys

import frondori_engine


def main() -> None:
    env_id = sys.argv[1] if len(sys.argv) > 1 else "football-v0"
    env = frondori_engine.make(env_id)

    # Seed fixe : relancer ce script rejoue exactement le même épisode.
    env.reset(seed=42)
    for agent in env.possible_agents:
        env.action_space(agent).seed(42)

    returns = {agent: 0.0 for agent in env.possible_agents}
    steps = 0
    while env.agents:
        actions = {agent: env.action_space(agent).sample() for agent in env.agents}
        _, rewards, _, _, _ = env.step(actions)
        for agent, reward in rewards.items():
            returns[agent] += reward
        steps += 1

    print(f"{env_id} : épisode terminé en {steps} pas, retours cumulés {returns}")


if __name__ == "__main__":
    main()
