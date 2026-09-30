"""Exécute un environnement dans un processus dédié, piloté par le serveur de
compétition (`frondori-server`) via l'entrée et la sortie standard.

Le serveur lance un worker par match (`python -m frondori_engine.worker`) :
un environnement qui plante ou se bloque n'emporte que son propre match, et
le serveur n'a pas à savoir en quel langage l'environnement est écrit.

Chaque message est un objet MessagePack précédé de sa taille (4 octets,
big-endian), dans les deux sens. Requêtes, selon `cmd` :
- `describe` : catalogue des environnements (agents, cadence, spaces) ;
- `start` (`env_id`, `seed`) : crée l'environnement et renvoie les
  observations initiales ;
- `step` (`actions` : agent -> action, ou `nil` si l'agent n'a pas répondu à
  temps) : un pas de simulation.
Réponses : `{"ok": true, ...}` ou `{"ok": false, "error": "..."}`.

Une action absente ou invalide est remplacée par l'action neutre de son
space (`wire.neutral_action`) ; les agents dont l'action a été refusée sont
listés dans `rejected`.
"""

from __future__ import annotations

import json
import os
import struct
import sys
import traceback

import msgpack

import frondori_engine
from frondori_engine import wire


def main() -> None:
    # La sortie standard porte le protocole : un `print` égaré dans le code
    # d'un environnement la corromprait. On garde une copie privée du vrai
    # stdout pour les réponses, et tout le reste part sur stderr.
    protocol_out = os.fdopen(os.dup(1), "wb")
    os.dup2(2, 1)
    sys.stdout = sys.stderr

    session = _Session()
    while (request := _read_frame(sys.stdin.buffer)) is not None:
        try:
            reply = {"ok": True, **session.handle(request)}
        except Exception:
            reply = {"ok": False, "error": traceback.format_exc()}
        _write_frame(protocol_out, reply)


class _Session:
    def __init__(self) -> None:
        self._env = None

    def handle(self, request: dict) -> dict:
        command = request.get("cmd")
        if command == "describe":
            return {"environments": {env_id: _describe(env_id) for env_id in frondori_engine.registered_ids()}}
        if command == "start":
            return self._start(request["env_id"], request["seed"])
        if command == "step":
            return self._step(request["actions"])
        raise ValueError(f"commande inconnue : {command!r}")

    def _start(self, env_id: str, seed: int) -> dict:
        self._env = frondori_engine.make(env_id, render_mode="scene")
        observations, infos = self._env.reset(seed=seed)
        return {
            "agents": list(self._env.possible_agents),
            "observations": wire.to_wire(observations),
            "infos": wire.to_wire(infos),
            "scene": self._scene(),
        }

    def _step(self, received: dict) -> dict:
        actions, rejected = {}, []
        for agent in self._env.agents:
            space = self._env.action_space(agent)
            value = received.get(agent)
            try:
                if value is None:
                    raise ValueError("pas d'action")
                actions[agent] = wire.from_wire(space, value)
            except (ValueError, TypeError):
                actions[agent] = wire.neutral_action(space)
                if value is not None:
                    rejected.append(agent)

        observations, rewards, terminations, truncations, infos = self._env.step(actions)
        return {
            "observations": wire.to_wire(observations),
            "rewards": {agent: float(reward) for agent, reward in rewards.items()},
            "terminations": {agent: bool(done) for agent, done in terminations.items()},
            "truncations": {agent: bool(done) for agent, done in truncations.items()},
            "infos": wire.to_wire(infos),
            "scene": self._scene(),
            "rejected": rejected,
        }

    def _scene(self) -> str:
        # Déjà en JSON : le serveur la relaie telle quelle aux spectateurs et
        # la concatène dans le replay, sans jamais avoir à la décoder.
        return json.dumps(self._env.render(), separators=(",", ":"))


def _describe(env_id: str) -> dict:
    env = frondori_engine.make(env_id)
    tick_rate = env.metadata.get("render_fps")
    if not tick_rate or tick_rate <= 0:
        raise ValueError(f"{env_id} : metadata['render_fps'] (cadence en pas/seconde) est requis")
    return {
        "agents": list(env.possible_agents),
        "tick_rate": float(tick_rate),
        "observation_spaces": {agent: wire.space_to_spec(env.observation_space(agent)) for agent in env.possible_agents},
        "action_spaces": {agent: wire.space_to_spec(env.action_space(agent)) for agent in env.possible_agents},
    }


def _read_frame(stream):
    header = stream.read(4)
    if len(header) < 4:
        return None
    (size,) = struct.unpack(">I", header)
    return msgpack.unpackb(stream.read(size), raw=False)


def _write_frame(stream, message: dict) -> None:
    payload = msgpack.packb(message, use_bin_type=True)
    stream.write(struct.pack(">I", len(payload)) + payload)
    stream.flush()


if __name__ == "__main__":
    main()
