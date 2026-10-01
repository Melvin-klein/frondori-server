"""Le worker, testé comme le serveur l'utilise : un vrai sous-processus,
piloté en MessagePack par son entrée et sa sortie standard."""

import json
import struct
import subprocess
import sys
import textwrap

import msgpack
import pytest

import frondori_engine

ZERO_FOOTBALL_ACTION = [[0.0] * 5] * 3


class Worker:
    def __init__(self, argv=None):
        self._process = subprocess.Popen(
            argv or [sys.executable, "-m", "frondori_engine.worker"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
        )

    def request(self, message: dict) -> dict:
        payload = msgpack.packb(message, use_bin_type=True)
        self._process.stdin.write(struct.pack(">I", len(payload)) + payload)
        self._process.stdin.flush()
        (size,) = struct.unpack(">I", self._process.stdout.read(4))
        return msgpack.unpackb(self._process.stdout.read(size), raw=False)

    def close(self) -> None:
        self._process.stdin.close()
        assert self._process.wait(timeout=10) == 0


@pytest.fixture
def worker():
    w = Worker()
    yield w
    w.close()


def test_describe_lists_every_registered_environment(worker):
    reply = worker.request({"cmd": "describe"})

    assert reply["ok"]
    environments = reply["environments"]
    assert sorted(environments) == frondori_engine.registered_ids()
    kitchen = environments["kitchen-v0"]
    assert kitchen["agents"] == ["chef_0", "chef_1"]
    assert kitchen["tick_rate"] == 5.0
    assert kitchen["action_spaces"]["chef_0"] == {"type": "discrete", "n": 6, "start": 0}
    assert kitchen["ranking"] == "mean_return"
    assert kitchen["title"] == "Cooperative Kitchen"
    assert environments["football-v0"]["ranking"] == "elo"
    football = environments["football-v0"]
    assert football["tick_rate"] == 30.0
    assert football["action_spaces"]["team_0"]["shape"] == [3, 5]
    assert football["observation_spaces"]["team_0"]["type"] == "dict"


def test_a_kitchen_episode_runs_through_the_worker(worker):
    start = worker.request({"cmd": "start", "env_id": "kitchen-v0", "seed": 0})

    assert start["ok"]
    assert start["agents"] == ["chef_0", "chef_1"]
    assert start["observations"]["chef_0"]["self"] == [1, 1, 0, 0]
    json.loads(start["scene"])

    # chef_0 se tourne vers la gauche (3) ; chef_1 n'a pas répondu (nil).
    step = worker.request({"cmd": "step", "actions": {"chef_0": 3, "chef_1": None}})

    assert step["ok"]
    assert step["rejected"] == []
    assert step["observations"]["chef_0"]["self"] == [1, 1, 2, 0]
    assert step["rewards"] == {"chef_0": 0.0, "chef_1": 0.0}


def test_invalid_actions_are_replaced_by_the_neutral_action(worker):
    worker.request({"cmd": "start", "env_id": "kitchen-v0", "seed": 0})

    step = worker.request({"cmd": "step", "actions": {"chef_0": 42, "chef_1": "n'importe quoi"}})

    assert step["ok"]
    assert sorted(step["rejected"]) == ["chef_0", "chef_1"]
    assert step["observations"]["chef_0"]["self"] == [1, 1, 0, 0]  # resté sur place


def test_football_actions_are_checked_against_their_box(worker):
    worker.request({"cmd": "start", "env_id": "football-v0", "seed": 0})

    out_of_bounds = worker.request(
        {"cmd": "step", "actions": {"team_0": ZERO_FOOTBALL_ACTION, "team_1": [[5.0] * 5] * 3}}
    )
    wrong_shape = worker.request(
        {"cmd": "step", "actions": {"team_0": [[0.0] * 5] * 2, "team_1": ZERO_FOOTBALL_ACTION}}
    )

    assert out_of_bounds["rejected"] == ["team_1"]
    assert wrong_shape["rejected"] == ["team_0"]


def test_an_error_is_reported_without_killing_the_worker(worker):
    reply = worker.request({"cmd": "start", "env_id": "chess-v0", "seed": 0})

    assert not reply["ok"]
    assert "chess-v0" in reply["error"]
    assert worker.request({"cmd": "describe"})["ok"]


def test_a_stray_print_in_an_environment_does_not_corrupt_the_protocol():
    bootstrap = textwrap.dedent("""
        import frondori_engine
        from frondori_engine.envs.kitchen import KitchenEnv

        class Noisy(KitchenEnv):
            def reset(self, seed=None, options=None):
                print("un print oublié dans un environnement")
                return super().reset(seed=seed, options=options)

        frondori_engine.register("noisy-v0", Noisy)

        from frondori_engine.worker import main
        main()
    """)
    worker = Worker([sys.executable, "-c", bootstrap])

    reply = worker.request({"cmd": "start", "env_id": "noisy-v0", "seed": 0})

    assert reply["ok"]
    worker.close()
