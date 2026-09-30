"""Le contrat commun, vérifié sur TOUS les environnements enregistrés : un
nouvel environnement est couvert automatiquement dès son `register(...)`."""

import json

import pytest
from pettingzoo.test import parallel_api_test, parallel_seed_test

import frondori_engine

ENV_IDS = frondori_engine.registered_ids()


@pytest.mark.parametrize("env_id", ENV_IDS)
def test_pettingzoo_parallel_api(env_id):
    parallel_api_test(frondori_engine.make(env_id), num_cycles=300)


@pytest.mark.parametrize("env_id", ENV_IDS)
def test_observations_always_fit_their_declared_space(env_id):
    # `parallel_api_test` ne le vérifie PAS (constaté : il laisse passer une
    # valeur hors bornes). Or c'est sur ces spaces que le serveur et les
    # agents vont s'appuyer : vérifié ici explicitement, pas par supposition.
    env = frondori_engine.make(env_id)
    observations, _ = env.reset(seed=0)
    for _ in range(500):
        for agent, observation in observations.items():
            assert env.observation_space(agent).contains(observation), (agent, observation)
        if not env.agents:
            break
        observations, *_ = env.step({agent: env.action_space(agent).sample() for agent in env.agents})


@pytest.mark.parametrize("env_id", ENV_IDS)
def test_same_seed_same_episode(env_id):
    parallel_seed_test(lambda: frondori_engine.make(env_id), num_cycles=100)


@pytest.mark.parametrize("env_id", ENV_IDS)
def test_scene_render_is_json_serializable(env_id):
    env = frondori_engine.make(env_id, render_mode="scene")
    env.reset(seed=0)
    env.step({agent: env.action_space(agent).sample() for agent in env.agents})

    scene = env.render()

    json.dumps(scene)
    assert scene["width"] > 0 and scene["height"] > 0
    assert scene["shapes"]
    assert {shape["type"] for shape in scene["shapes"]} <= {"rect", "circle", "text"}
