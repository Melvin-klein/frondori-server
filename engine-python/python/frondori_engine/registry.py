"""Registre des environnements, sur le modèle de `gymnasium.make`.

Tout identifiant porte une version (`nom-vN`). Changer les règles d'un
environnement, c'est en publier une nouvelle version, jamais modifier
l'ancienne : un résultat, un replay ou un classement obtenu sur
`football-v0` doit rester comparable dans six mois.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any, Callable

from pettingzoo import ParallelEnv

_ID_PATTERN = re.compile(r"^[a-z][a-z0-9_]*-v\d+$")


@dataclass(frozen=True)
class EnvSpec:
    id: str
    entry_point: Callable[..., ParallelEnv]
    kwargs: dict[str, Any] = field(default_factory=dict)


_registry: dict[str, EnvSpec] = {}


def register(id: str, entry_point: Callable[..., ParallelEnv], **kwargs: Any) -> None:
    """Enregistre un environnement. `kwargs` : paramètres par défaut passés à
    `entry_point`, surchargeables au moment de `make`."""
    if not _ID_PATTERN.match(id):
        raise ValueError(f"identifiant invalide {id!r} : format attendu 'nom-vN' (ex. 'football-v0')")
    if id in _registry:
        raise ValueError(f"environnement {id!r} déjà enregistré")
    _registry[id] = EnvSpec(id=id, entry_point=entry_point, kwargs=kwargs)


def make(id: str, **kwargs: Any) -> ParallelEnv:
    try:
        spec = _registry[id]
    except KeyError:
        known = ", ".join(sorted(_registry)) or "(aucun)"
        raise KeyError(f"environnement inconnu {id!r} ; disponibles : {known}") from None
    return spec.entry_point(**{**spec.kwargs, **kwargs})


def registered_ids() -> list[str]:
    return sorted(_registry)
