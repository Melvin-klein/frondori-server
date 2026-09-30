# Frondori — plateforme de compétition et de recherche en IA

Plateforme où des participants (étudiants, chercheurs, développeurs) hébergent
leur propre modèle chez eux et se connectent via un SDK, en WebSocket, pour
le faire jouer dans des environnements de simulation. Le premier est un
football 2D ; d'autres, sans aucun rapport, vont s'ajouter au fil des
questions de recherche (cf. `engine-python` ci-dessous). Objectif :
performance temps réel + architecture simple à maintenir.

## Vue d'ensemble

Un workspace Cargo de quatre crates. Le SDK de compétition vit dans un dépôt
séparé (`frondori-sdk-python`), et le site dans `frondori-web` (Laravel, même
base Postgres).

```
frondori-server/
├── engine/         moteur physique du football (rapier2d), pur, sans réseau
├── protocol/       types de messages serveur <-> SDK, encodage MessagePack
├── server/         binaire principal : gateway (auth+matchmaking) + match runner
├── engine-python/  environnements de recherche (PettingZoo), dont le football
│                   via des bindings PyO3 d'`engine` ; publié sur PyPI (`frondori-engine`)
└── viewer/         visionneuse spectateur locale (HTML statique)
```

Les crates Rust communiquent par appel de fonction direct (dépendances
`path = "../..."`), jamais par réseau/IPC. Le SDK, lui, parle au serveur
exclusivement via WebSocket + MessagePack — c'est la seule frontière réseau
du projet.

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
après la dernière observation, ET un échec de fermeture de sa propre
connexion (cf. `Agent.play` dans le SDK Python pour l'exemple de référence).

Deux exemples utiles pour tester sans attendre un match réseau de 5 minutes :
`server/examples/seed_test_match.rs` (match complet simulé hors réseau et
persisté dans Postgres) et `server/examples/short_test_server.rs` (vrai
serveur réseau, matchs de 2 secondes, auth en mémoire).

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

## `engine-python` — environnements de recherche

Projet mixte Rust + Python (maturin) : le paquet public est
`python/frondori_engine/`, le module natif `frondori_engine._football`
(compilé depuis `src/lib.rs`) n'est qu'un détail d'implémentation du football.

**Le contrat commun, c'est PettingZoo** (`ParallelEnv`, spaces Gymnasium),
pas les structs de `protocol`. Choix délibéré : les futurs environnements
peuvent n'avoir aucun rapport avec le football (ex. `kitchen-v0`, cuisine
coopérative à la Overcooked, écrite en Python pur pour le vérifier). Tout
environnement :
- est enregistré sous un identifiant versionné `nom-vN` (`registry.py`).
  Changer des règles = publier `nom-v(N+1)`, jamais modifier `nom-vN`
  (reproductibilité des résultats, replays et classements) ;
- se rend via `render_mode="scene"` : primitives génériques rect/cercle/texte
  en JSON (`scene.py`), pour qu'un seul afficheur dessine tous les jeux ;
- est couvert automatiquement par `tests/test_contract.py` dès son
  `register(...)` : API PettingZoo, déterminisme par seed, observations
  conformes à leur space. **Piège** : le `parallel_api_test` officiel ne
  vérifie PAS l'appartenance des observations à leur space (constaté avec un
  contrôle négatif) — d'où le test explicite.

`.cargo/config.toml` ajoute des flags de lien macOS : sans eux, la feature
`extension-module` de PyO3 casse `cargo build/test --workspace` sur macOS.

Dev : `cd engine-python && source .venv/bin/activate && maturin develop &&
python -m pytest`. Un simple `cargo build -p engine-python` ne produit pas un
module importable : toujours passer par maturin.

## Commandes utiles

```bash
# Rust : tout le workspace
cargo test --workspace
cargo run -p engine --example random_agent

# Rust : avec Postgres (sinon fallback mémoire)
DATABASE_URL="postgres://melvine@localhost/frondori" cargo test --workspace

# Environnements de recherche (Python)
cd engine-python && source .venv/bin/activate && maturin develop && python -m pytest
python examples/random_agent.py kitchen-v0
```

## État du projet / ce qui reste

Fait et validé de bout en bout (vrais sockets, vraie base Postgres) :
`engine`, `protocol`, `server` (gateway, match runner, auth, persistance des
matchs + replays, flux spectateur `/spectate`), SDK Python (repo séparé).
L'ELO est calculé côté `frondori-web` (commande `matches:update-ratings`).

Chantier en cours, multi-environnements :
- phase 1 (faite) : contrat PettingZoo, registre versionné, rendu en scène,
  `football-v0` et `kitchen-v0` en local ;
- phase 2 (à faire) : protocole, serveur, SDK et site sur ce MÊME contrat.
  Décidé : le serveur exécute chaque environnement dans un **processus séparé**
  (indépendant du langage), le protocole transporte un identifiant
  d'environnement + des valeurs validées contre les spaces, la compatibilité
  avec l'ancien protocole football n'est pas à préserver, le classement se
  choisit par type d'environnement (ELO en duel, score partagé en coopératif).

Hors scope pour l'instant : règles avancées du football (hors-jeu, fautes),
reconnexion en cours de match, CI.

## Convention de travail sur ce projet

L'utilisateur découvre Rust (à l'aise en C/Python). Commenter le code Rust
de façon appuyée : expliquer le POURQUOI des choix (ownership, async,
traits...) autant que le code lui-même, pas juste ce qu'un identifieur bien
nommé dit déjà. Avant de déclarer une fonctionnalité réseau "terminée",
valider avec un vrai test de bout en bout (vrais sockets/vrai process), pas
seulement `cargo check`/tests unitaires — plusieurs bugs réels de ce projet
(race de fin de match, format MessagePack, collision de coup d'envoi) n'ont
été trouvés qu'en testant en conditions réelles.
