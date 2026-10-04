# syntax=docker/dockerfile:1
#
# Image du serveur de jeu : le binaire Rust + le Python des workers, avec les
# environnements proposés, installés depuis PyPI. Le serveur propose
# exactement ces environnements-là.
#
#   docker build -t frondori-game .
#   docker build --build-arg ENVIRONMENTS="frondori-engine==0.4.0 frondori-kitchen==0.1.0" -t frondori-game .
#
# Utilisée par le dépôt de déploiement (Docker Compose), qui fixe la liste des
# environnements et les limites de ressources.

# --- 1. Compilation -----------------------------------------------------------
FROM rust:1-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY protocol protocol
COPY server server
# Caches de compilation conservés d'un build à l'autre (dépendances déjà
# compilées) : seul ce qui a changé est recompilé. Le dossier `target` étant
# un cache, les binaires sont copiés hors de lui dans la même commande.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p server --bin frondori-server --bin manage-tokens \
    && mkdir /out \
    && cp target/release/frondori-server target/release/manage-tokens /out/

# --- 2. Image finale -----------------------------------------------------------
FROM python:3.13-slim-trixie

# Les environnements joués, versions fixées : une image reconstruite plus tard
# joue exactement les mêmes règles.
ARG ENVIRONMENTS="frondori-engine==0.4.0 frondori-kitchen==0.1.0 frondori-football==0.1.0"

# Le Python des workers, dans son propre venv. La dernière commande échoue si
# un environnement ne se charge pas : mieux vaut un build cassé qu'un serveur
# qui refuse de démarrer.
RUN python -m venv /opt/worker \
    && /opt/worker/bin/pip install --no-cache-dir ${ENVIRONMENTS} \
    && /opt/worker/bin/python -c "import frondori_engine; print('environnements :', frondori_engine.registered_ids())"

COPY --from=build /out/ /usr/local/bin/

# Utilisateur sans privilèges : le code des environnements s'exécute ici.
RUN useradd --system --uid 10001 --no-create-home frondori
USER frondori

# Un worker = un match, et un pas ne prend que quelques millisecondes : les
# pools de threads de numpy (un thread par cœur sinon) ne feraient que
# multiplier les processus légers. Le serveur transmet ces variables au worker.
ENV FRONDORI_ENV_WORKER="/opt/worker/bin/python -m frondori_engine.worker" \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    OMP_NUM_THREADS=1 \
    OPENBLAS_NUM_THREADS=1 \
    MKL_NUM_THREADS=1

EXPOSE 8080
CMD ["frondori-server"]
