# Frondori — plateforme de compétition d'IA (football 2D)

Plateforme où des participants (étudiants, chercheurs, développeurs) hébergent
leur propre modèle chez eux et se connectent via un SDK, en WebSocket, pour
faire jouer leur modèle sur un jeu de football 2D simplifié. Objectif :
performance temps réel + architecture simple à maintenir.

**Ce dépôt n'est pas (encore) un dépôt git.** Pas de commits, pas d'historique
à consulter — l'état du code sur disque est la seule source de vérité.

## Vue d'ensemble

Un seul workspace Cargo (trois crates, compilées ensemble, un seul binaire
final) + un SDK Python séparé.

```
frondori-server/
├── engine/     moteur de simulation physique (rapier2d), pur, sans réseau
├── protocol/   types de messages partagés serveur <-> SDK, encodage MessagePack
├── server/     binaire principal : gateway (auth+matchmaking) + match runner
└── sdk/python/ SDK Python pour les participants (package séparé, pas dans le workspace Cargo)
```

Les trois crates Rust communiquent par appel de fonction direct (dépendances
`path = "../..."`), jamais par réseau/IPC. Le SDK Python, lui, parle au
serveur exclusivement via WebSocket + MessagePack — c'est la seule frontière
réseau du projet.

## `engine` — moteur de simulation

API façon OpenAI Gym : `Engine::new(config, seed)`, puis `reset()` /
`step(actions)`. **Déterminisme total** exigé : même seed + mêmes actions =
même résultat, tick pour tick (rapier2d + RNG seedé, jamais de
`rand::thread_rng()` dans la logique de simulation elle-même).

Décisions à connaître avant de toucher à `engine/src/sim.rs` :
- **1 socket = 1 équipe** : `Actions` transporte les actions de TOUS les
  joueurs d'une équipe (pas un joueur = une connexion).
- **Observation symétrique/mirroir** : chaque équipe voit toujours
  `self_team` comme la sienne et attaque toujours vers `x = +1`, quel que
  soit son côté physique réel sur le terrain (le moteur fait le mirroring en
  interne, cf. doc d'`Observation` dans `engine/src/types.rs`).
- **Pas de murs sur les lignes de but** (seulement sur les lignes de
  touche haut/bas) : un but est détecté "à la main" après chaque pas de
  physique (`resolve_ball_bounds`), pas via une collision rapier2d.
- **Formation de coup d'envoi** : le joueur n°0 de chaque équipe démarre
  juste à côté du ballon, l'un en face de l'autre. Un tir immédiat au coup
  d'envoi percute le joueur adverse avant d'atteindre le but — c'est
  volontaire (réaliste), mais ça a surpris un test avant d'être compris (cf.
  `engine/tests/goal_detection.rs`, qui dégage d'abord le joueur adverse).
- Actions par défaut en cas de timeout réseau : `Action::NOOP`, jamais la
  dernière action reçue (décision documentée dans `match_runner`).

Tests : `engine/tests/{determinism,goal_detection,stability}.rs` +
`engine/examples/random_agent.rs`. Lancer avec `cargo test -p engine`.

## `protocol` — messages réseau

`Hello/Welcome/AuthError/ActionMessage/ObservationMessage/MatchEnd/Ping/Pong`,
réutilisant directement `engine::types::{Action, Actions, Observation}`.

**Format d'encodage MessagePack (important, vérifié empiriquement contre un
décodeur Python — ne pas changer sans mettre à jour le SDK) :**
- `protocol::encode` utilise `rmp_serde::to_vec_named` : les structs sont des
  MAPS `{"champ": valeur}`, pas des tableaux positionnels.
- Les enums à données (`ServerMessage`, `ClientMessage`) sont des maps à une
  seule clé : `{"Welcome": {...}}`.
- Les enums SANS données (`MatchOutcome`) sont la chaîne du nom du variant
  directement : `"Draw"`, pas `{"Draw": null}`.

## `server` — gateway + match runner

Package Cargo avec un **lib + deux bins** (important : `server` est aussi une
bibliothèque, pas juste un binaire — c'est ce qui permet aux tests
d'intégration de démarrer un vrai serveur en mémoire) :
- `frondori-server` (`src/main.rs`) : le serveur lui-même.
- `manage-tokens` (`src/bin/manage_tokens.rs`) : CLI d'administration des
  tokens (`add` / `list` / `remove`), parle directement à Postgres.

**`gateway`** (`src/gateway/`) : handshake (Hello -> Welcome/AuthError) +
matchmaking FIFO. File protégée par `std::sync::Mutex` (pas le mutex async de
tokio — aucune opération `.await` n'est faite verrou tenu). Avant de spawn un
match avec un joueur qui attendait en file, un ping applicatif vérifie qu'il
est toujours connecté ; sinon on le retire et on retente avec le suivant.

**`match_runner`** (`src/match_runner/`) : boucle à tick fixe
(`tokio::time::interval`, 30 Hz par défaut, `MissedTickBehavior::Delay`).
Timeout d'action -> NOOP. Déconnexion -> forfait immédiat, pas de
reconnexion. **Piège connu** : le serveur envoie l'observation du DERNIER
tick puis ferme la connexion sans attendre de réponse (il sait déjà que le
match est fini) — n'importe quel client doit tolérer un échec d'envoi juste
après la dernière observation (cf. `ConnectionLostError`/le `try/except`
dans le SDK Python pour l'exemple de référence).

**`auth`** (`src/auth/`) : trait `AuthProvider` avec deux implémentations :
- `InMemoryAuthProvider` : table en mémoire, utilisée par défaut si
  `DATABASE_URL` n'est pas définie (et par les tests). Aucun token ne
  survit à un redémarrage.
- `PostgresAuthProvider` (`src/auth/postgres.rs`) : utilisée si
  `DATABASE_URL` est définie. Table `players (token, player_id,
  display_name, created_at)`, créée automatiquement (`CREATE TABLE IF NOT
  EXISTS`, pas de système de migration séparé). Une erreur de connexion à ce
  stade est **fatale** au démarrage (volontaire : mieux vaut planter que
  servir silencieusement sans aucun token valide).

### Setup PostgreSQL local (dev)

Postgres 18 installé via Homebrew, service démarré. Base `frondori` déjà
créée. **Piège** : `postgres:///frondori` (sans utilisateur explicite)
échoue avec `role "anonymous" does not exist` — toujours mettre l'utilisateur
dans l'URL :

```bash
export DATABASE_URL="postgres://melvine@localhost/frondori"
cargo run -p server --bin manage-tokens -- add <token> <player_id> "<nom>"
cargo run -p server --bin manage-tokens -- list
cargo run -p server --bin frondori-server
```

Sans `DATABASE_URL`, le serveur démarre quand même (auth en mémoire, table
vide, aucun token accepté — utile pour un `cargo run` rapide, jamais pour un
usage réel).

## `sdk/python` — SDK participant

Package Python indépendant (pas dans le workspace Cargo), voir
`sdk/python/README.md` pour l'usage. API callback : `Agent(url, token).run(act)`
(bloquant, script classique) ou `await agent.play(act)` (notebook
Jupyter/code déjà async). Erreurs dédiées : `AuthenticationError`,
`ConnectionLostError`, `ProtocolError`.

Environnement de dev : `cd sdk/python && python -m venv .venv && source
.venv/bin/activate && pip install -e ".[dev]"`.

## Commandes utiles

```bash
# Rust : tout le workspace
cargo test --workspace
cargo run -p engine --example random_agent

# Rust : avec Postgres (sinon fallback mémoire)
DATABASE_URL="postgres://melvine@localhost/frondori" cargo test --workspace

# Python
cd sdk/python && source .venv/bin/activate && python -m pytest
```

## État du projet / ce qui reste

Fait et validé de bout en bout (vrais sockets, vraie base Postgres) :
`engine`, `protocol`, `server` (gateway+match runner+auth mémoire/Postgres),
SDK Python.

Pas fait, volontairement hors scope V1 : classement/ELO, spectateurs/replay,
règles avancées (hors-jeu, fautes), reconnexion en cours de match.

Pas fait, à faire si besoin : persistance des résultats de match (juste
loggés actuellement), SDK dans un autre langage que Python, CI, dépôt git.

## Convention de travail sur ce projet

L'utilisateur découvre Rust (à l'aise en C/Python). Commenter le code Rust
de façon appuyée : expliquer le POURQUOI des choix (ownership, async,
traits...) autant que le code lui-même, pas juste ce qu'un identifieur bien
nommé dit déjà. Avant de déclarer une fonctionnalité réseau "terminée",
valider avec un vrai test de bout en bout (vrais sockets/vrai process), pas
seulement `cargo check`/tests unitaires — plusieurs bugs réels de ce projet
(race de fin de match, format MessagePack, collision de coup d'envoi) n'ont
été trouvés qu'en testant en conditions réelles.
