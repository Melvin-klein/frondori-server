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
def test_declares_a_tick_rate(env_id):
    # Cadence d'un match en compétition : le serveur en a besoin pour jouer
    # l'environnement en réseau (cf. `worker._describe`).
    assert frondori_engine.make(env_id).metadata["render_fps"] > 0


@pytest.mark.parametrize("env_id", ENV_IDS)
def test_declares_a_compute_budget(env_id):
    # Temps de calcul accordé à un agent pour chaque action en compétition
    # (cf. `worker._describe`) : le serveur l'applique, le SDK l'annonce.
    assert frondori_engine.make(env_id).metadata["compute_budget_ms"] > 0


@pytest.mark.parametrize("env_id", ENV_IDS)
def test_declares_how_agents_are_ranked(env_id):
    # Le site classe les agents selon ce type, sans rien connaître du jeu.
    env = frondori_engine.make(env_id)
    assert env.metadata["ranking"] in ("elo", "mean_return")
    if env.metadata["ranking"] == "elo":
        assert len(env.possible_agents) == 2
    assert env.metadata["title"]
    # Règles en Markdown, affichées telles quelles par le site.
    assert env.metadata["documentation"].strip()


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
