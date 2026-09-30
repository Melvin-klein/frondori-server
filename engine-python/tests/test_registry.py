import pytest

import frondori_engine
from frondori_engine.registry import register


def test_make_passes_kwargs_to_the_environment():
    env = frondori_engine.make("kitchen-v0", max_steps=3)
    env.reset()

    for _ in range(3):
        *_, truncations, _ = env.step({agent: 0 for agent in env.agents})

    assert all(truncations.values())
    assert env.agents == []


def test_unknown_environment_lists_available_ones():
    with pytest.raises(KeyError, match="football-v0"):
        frondori_engine.make("chess-v0")


@pytest.mark.parametrize("bad_id", ["football", "Football-v0", "football-v", "football_v0"])
def test_ids_must_be_versioned(bad_id):
    with pytest.raises(ValueError, match="nom-vN"):
        register(bad_id, frondori_engine.KitchenEnv)


def test_an_id_cannot_be_registered_twice():
    with pytest.raises(ValueError, match="déjà enregistré"):
        register("kitchen-v0", frondori_engine.KitchenEnv)
