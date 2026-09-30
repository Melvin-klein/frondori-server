import numpy as np
import pytest

import frondori_engine

# Chaque joueur fonce vers le but adverse et tire dans cette direction.
ATTACK = np.tile(np.array([1.0, 0.0, 1.0, 0.0, 1.0], dtype=np.float32), (3, 1))
IDLE = np.zeros((3, 5), dtype=np.float32)


def test_a_goal_rewards_the_scorer_and_penalizes_the_other_team():
    env = frondori_engine.make("football-v0")
    env.reset(seed=1)

    for _ in range(9000):
        observations, rewards, *_ = env.step({"team_0": ATTACK, "team_1": IDLE})
        if rewards["team_0"] != 0:
            break

    assert rewards == {"team_0": 1.0, "team_1": -1.0}
    assert list(observations["team_0"]["score"]) == [1, 0]
    # Chaque équipe voit le score de SON point de vue : (marqués, encaissés).
    assert list(observations["team_1"]["score"]) == [0, 1]


def test_time_limit_is_a_truncation():
    env = frondori_engine.make("football-v0", max_ticks=5)
    env.reset(seed=0)

    for _ in range(5):
        _, _, terminations, truncations, _ = env.step({"team_0": IDLE, "team_1": IDLE})

    assert truncations == {"team_0": True, "team_1": True}
    assert terminations == {"team_0": False, "team_1": False}
    assert env.agents == []


def test_reaching_max_score_is_a_termination():
    env = frondori_engine.make("football-v0", max_score=1)
    env.reset(seed=1)

    while env.agents:
        _, rewards, terminations, truncations, _ = env.step({"team_0": ATTACK, "team_1": IDLE})

    assert rewards["team_0"] == 1.0
    assert terminations == {"team_0": True, "team_1": True}
    assert truncations == {"team_0": False, "team_1": False}


def test_an_action_with_the_wrong_number_of_players_is_rejected():
    env = frondori_engine.make("football-v0")
    env.reset(seed=0)

    with pytest.raises(ValueError, match=r"\(3, 5\)"):
        env.step({"team_0": np.zeros((2, 5)), "team_1": IDLE})
