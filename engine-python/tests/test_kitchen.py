"""Plan par défaut, en (ligne, colonne) :

    XXPXX      chef_0 part de (1, 1), chef_1 de (1, 3)
    O1 2S      oignons en (1, 0), marmite en (0, 2), passe en (1, 4)
    X   X      assiettes en (3, 1)
    XDXXX
"""

import pytest

import frondori_engine
from frondori_engine.envs.kitchen import (
    INTERACT,
    MOVE_DOWN,
    MOVE_LEFT,
    MOVE_RIGHT,
    MOVE_UP,
    ONION,
    SOUP,
    STAY,
)


def play(env, script):
    """`script` : liste de paires (action chef_0, action chef_1), une par pas."""
    results = []
    for action_0, action_1 in script:
        results.append(env.step({"chef_0": action_0, "chef_1": action_1}))
    return results


def test_serving_a_soup_rewards_both_chefs():
    env = frondori_engine.make("kitchen-v0", onions_needed=2, cook_time=5)
    env.reset()

    # chef_1 s'écarte du passe ; chef_0 fait toute la recette seul.
    chef_0 = [
        MOVE_LEFT, INTERACT,              # oignon n°1
        MOVE_RIGHT, MOVE_UP, INTERACT,    # dans la marmite
        MOVE_LEFT, INTERACT,              # oignon n°2
        MOVE_RIGHT, MOVE_UP, INTERACT,    # dans la marmite -> cuisson
        MOVE_DOWN, MOVE_LEFT, MOVE_DOWN, INTERACT,  # assiette
        MOVE_UP, MOVE_RIGHT, MOVE_UP, INTERACT,     # soupe récupérée
        MOVE_RIGHT, INTERACT,             # servie au passe
    ]
    chef_1 = [MOVE_DOWN] + [STAY] * (len(chef_0) - 1)

    results = play(env, zip(chef_0, chef_1))

    rewards = [r[1] for r in results]
    assert rewards[-1] == {"chef_0": 1.0, "chef_1": 1.0}
    assert all(r == {"chef_0": 0.0, "chef_1": 0.0} for r in rewards[:-1])
    assert results[-1][4]["chef_0"]["served"] == 1


def test_soup_cannot_be_taken_before_it_is_cooked():
    env = frondori_engine.make("kitchen-v0", onions_needed=1, cook_time=50)
    env.reset()

    results = play(env, [
        (MOVE_LEFT, STAY), (INTERACT, STAY),
        (MOVE_RIGHT, STAY), (MOVE_UP, STAY), (INTERACT, STAY),   # marmite pleine, cuisson lancée
        (MOVE_DOWN, STAY), (MOVE_LEFT, STAY), (MOVE_DOWN, STAY), (INTERACT, STAY),  # assiette
        (MOVE_UP, STAY), (MOVE_RIGHT, STAY), (MOVE_UP, STAY), (INTERACT, STAY),     # trop tôt
    ])

    observation = results[-1][0]["chef_0"]
    assert observation["self"][3] != SOUP
    assert observation["pots"][0][0] == 1  # l'oignon est toujours dans la marmite


def test_an_item_can_be_handed_over_through_a_counter():
    env = frondori_engine.make("kitchen-v0")
    env.reset()

    results = play(env, [
        (MOVE_LEFT, STAY), (INTERACT, STAY),     # chef_0 prend un oignon
        (MOVE_UP, STAY), (INTERACT, STAY),       # et le pose sur le plan (0, 1)
        (MOVE_DOWN, MOVE_LEFT),                  # chef_0 libère la place
        (STAY, MOVE_LEFT), (STAY, MOVE_UP),      # chef_1 se place face au plan
        (STAY, INTERACT),                        # et ramasse l'oignon
    ])

    observation = results[-1][0]["chef_1"]
    assert observation["self"][3] == ONION
    assert not observation["counters"].any()


def test_chefs_cannot_walk_into_each_other():
    env = frondori_engine.make("kitchen-v0")
    env.reset()

    # Les deux visent la case du milieu (1, 2) au même pas : aucun ne bouge.
    (observations, *_), = play(env, [(MOVE_RIGHT, MOVE_LEFT)])

    assert list(observations["chef_0"]["self"][:2]) == [1, 1]
    assert list(observations["chef_1"]["self"][:2]) == [1, 3]


def test_each_chef_sees_itself_as_self_and_the_other_as_partner():
    env = frondori_engine.make("kitchen-v0")
    observations, _ = env.reset()

    assert list(observations["chef_0"]["self"][:2]) == [1, 1]
    assert list(observations["chef_0"]["partner"][:2]) == [1, 3]
    assert list(observations["chef_1"]["self"][:2]) == [1, 3]
    assert list(observations["chef_1"]["partner"][:2]) == [1, 1]


def test_invalid_action_is_rejected():
    env = frondori_engine.make("kitchen-v0")
    env.reset()

    with pytest.raises(ValueError, match="entre 0 et 5"):
        env.step({"chef_0": 9, "chef_1": STAY})
