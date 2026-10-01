#!/usr/bin/env bash
# Prépare le Python qui exécute les environnements pour ce serveur (le
# "worker", cf. server/src/environments.rs) : un venv `.venv` à la racine de
# ce dépôt, avec frondori-engine et les environnements à proposer.
#
# Le serveur ne contient aucun jeu : il joue tous les environnements
# installés dans ce Python, et seulement eux. Proposer un nouvel
# environnement = l'installer ici, puis redémarrer le serveur.
#
# Usage : scripts/setup-environments.sh [DOSSIER_DES_DÉPÔTS]
#   (par défaut, les dépôts frères : ../frondori-engine, ../frondori-kitchen...)
# Le football se compile depuis ses sources Rust : il faut une toolchain Rust.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
repos="${1:-$(dirname "$root")}"

python3 -m venv "$root/.venv"
"$root/.venv/bin/pip" install --quiet --upgrade pip
"$root/.venv/bin/pip" install --quiet maturin
"$root/.venv/bin/pip" install --quiet \
    -e "$repos/frondori-engine" \
    -e "$repos/frondori-kitchen" \
    -e "$repos/frondori-football"

"$root/.venv/bin/python" -c "import frondori_engine; print('environnements installés :', ', '.join(frondori_engine.registered_ids()))"
echo "Worker : export FRONDORI_ENV_WORKER=\"$root/.venv/bin/python -m frondori_engine.worker\""
