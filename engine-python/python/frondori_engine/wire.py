"""Passage entre les valeurs d'un environnement (numpy, spaces Gymnasium) et
leur forme sur le fil (types MessagePack natifs : nombres, listes, dicts).

Ce format fait partie du protocole de compétition : le SDK
(`frondori-sdk-python`) en implémente l'autre moitié (spec -> space, fil ->
numpy). Toute modification ici doit y être reportée.

Spaces supportés : Box, Discrete, MultiDiscrete, et Dict de ces spaces. Un
environnement qui en utilise un autre est refusé explicitement à la
description (`space_to_spec`), pas silencieusement mal transmis.
"""

from __future__ import annotations

import numpy as np
from gymnasium import spaces


def space_to_spec(space: spaces.Space) -> dict:
    if isinstance(space, spaces.Box):
        return {
            "type": "box",
            "shape": list(space.shape),
            "dtype": space.dtype.name,
            "low": space.low.tolist(),
            "high": space.high.tolist(),
        }
    if isinstance(space, spaces.Discrete):
        return {"type": "discrete", "n": int(space.n), "start": int(space.start)}
    if isinstance(space, spaces.MultiDiscrete):
        return {
            "type": "multi_discrete",
            "nvec": space.nvec.tolist(),
            "start": space.start.tolist(),
            "dtype": space.dtype.name,
        }
    if isinstance(space, spaces.Dict):
        return {"type": "dict", "spaces": {key: space_to_spec(sub) for key, sub in space.spaces.items()}}
    raise TypeError(f"space non supporté par le protocole : {type(space).__name__}")


def to_wire(value):
    """numpy -> types natifs, récursivement (observations, infos)."""
    if isinstance(value, np.ndarray):
        return value.tolist()
    if isinstance(value, np.generic):
        return value.item()
    if isinstance(value, dict):
        return {str(key): to_wire(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [to_wire(item) for item in value]
    return value


def from_wire(space: spaces.Space, value):
    """Action reçue du fil -> valeur du space. Lève `ValueError` si elle n'y
    appartient pas : un agent n'est pas de confiance, rien n'est passé à
    l'environnement sans vérification."""
    if isinstance(space, spaces.Dict):
        if not isinstance(value, dict) or set(value) != set(space.spaces):
            raise ValueError(f"attendu un dict avec les clés {sorted(space.spaces)}")
        return {key: from_wire(sub, value[key]) for key, sub in space.spaces.items()}
    if isinstance(space, spaces.Discrete):
        # `bool` est un sous-type d'`int` en Python : refusé explicitement.
        if isinstance(value, bool) or not isinstance(value, int):
            raise ValueError("attendu un entier")
        action = np.int64(value)
    else:
        action = np.asarray(value, dtype=space.dtype)
    if not space.contains(action):
        raise ValueError("valeur hors de l'action_space")
    return action


def neutral_action(space: spaces.Space):
    """Action appliquée à un agent qui n'a pas répondu à temps, ou dont
    l'action est invalide : l'élément « zéro » du space (0 ramené dans les
    bornes, ou la plus petite valeur discrète). Un environnement doit donc
    être conçu pour que cet élément soit une action sans effet — c'est le cas
    du football (ne pas bouger, ne pas tirer) et de la cuisine (rester)."""
    if isinstance(space, spaces.Dict):
        return {key: neutral_action(sub) for key, sub in space.spaces.items()}
    if isinstance(space, spaces.Discrete):
        return np.int64(space.start)
    if isinstance(space, spaces.MultiDiscrete):
        return np.array(space.start, dtype=space.dtype)
    if isinstance(space, spaces.Box):
        return np.clip(np.zeros(space.shape), space.low, space.high).astype(space.dtype)
    raise TypeError(f"space non supporté par le protocole : {type(space).__name__}")
