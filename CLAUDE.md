# Frondori — plateforme de compétition et de recherche en IA

Plateforme où des participants (étudiants, chercheurs, développeurs) hébergent
leur propre modèle chez eux et se connectent via un SDK, en WebSocket, pour
le faire jouer dans des environnements de simulation. Le premier est un
football 2D ; d'autres, sans aucun rapport, vont s'ajouter au fil des
questions de recherche. Objectif : performance temps réel + architecture
simple à maintenir, et ajouter un environnement sans toucher au serveur.

## Vue d'ensemble

Un workspace Cargo de quatre crates. Le SDK de compétition vit dans un dépôt
séparé (`frondori-sdk-python`), et le site dans `frondori-web` (Laravel, même
base Postgres).

```
frondori-server/
├── engine/         moteur physique du football (rapier2d), pur, sans réseau
├── engine-python/  LES environnements (PettingZoo) + le worker qui les exécute
│                   pour le serveur ; publié sur PyPI (`frondori-engine`)
├── protocol/       messages serveur <-> SDK, encodage MessagePack, AUCUN jeu
├── server/         gateway (auth + matchmaking) + match runner, AUCUNE règle de jeu
└── viewer/         visionneuse spectateur locale (HTML statique, tout environnement)
```

**Le serveur ne contient aucune règle de jeu.** Chaque match lance un
worker Python (`python -m frondori_engine.worker`, un processus par match)
qui exécute l'environnement ; le serveur relaie observations et actions
entre les participants et ce worker. Le football (Rust) y passe comme les
autres, via ses bindings PyO3 : coût mesuré ~0,2 ms par pas, 0,6 % du budget
d'un tick à 30 Hz. `server` ne dépend plus du crate `engine`.

## `engine-python` — les environnements

Projet mixte Rust + Python (maturin) : le paquet public est
`python/frondori_engine/`, le module natif `frondori_engine._football`
(compilé depuis `src/lib.rs`) n'est qu'un détail d'implémentation du football.

**Le contrat commun, c'est PettingZoo** (`ParallelEnv`, spaces Gymnasium).
Tout environnement :
- est enregistré sous un identifiant versionné `nom-vN` (`registry.py`).
  Changer des règles = publier `nom-v(N+1)`, jamais modifier `nom-vN`
  (reproductibilité des résultats, replays et classements) ;
- déclare sa cadence en compétition dans `metadata["render_fps"]` (pas par
  seconde : 30 pour le football, 5 pour la cuisine) ;
- déclare `metadata["title"]`, `metadata["description"]` (affichés par le
  site) et `metadata["ranking"]` : `"elo"` (duel à 2 agents uniquement — le
  vainqueur est l'agent au meilleur retour) ou `"mean_return"` (retour moyen
  par match). Vérifié par le contrat et par `worker._describe` ;
- peut renvoyer dans ses infos une clé `score` : convention que le site
  affiche comme score du match (le football y met ses buts marqués) ; à
  défaut, le site affiche le retour. Toutes les infos numériques finales
  apparaissent dans les statistiques de la page de match ;
- se rend via `render_mode="scene"` : primitives génériques rect/cercle/texte
  en JSON (`scene.py`), qu'un seul afficheur dessine pour tous les jeux ;
- doit être conçu pour que l'élément « zéro » de son `action_space` soit une
  action sans effet : c'est l'action neutre jouée quand un agent ne répond
  pas à temps ou envoie une action invalide (`wire.neutral_action`) ;
- est couvert automatiquement par `tests/test_contract.py` dès son
  `register(...)`. **Piège** : le `parallel_api_test` officiel ne vérifie PAS
  l'appartenance des observations à leur space (constaté avec un contrôle
  négatif) — d'où le test explicite.

Ajouter un environnement = un module dans `envs/` + une ligne `register(...)`
dans `__init__.py`. Rien à changer dans `protocol`, `server` ni le SDK.

**Worker** (`worker.py`) : piloté par le serveur sur stdin/stdout, trames
MessagePack préfixées de leur taille (4 octets big-endian). Commandes
`describe` (catalogue, appelée une fois au démarrage du serveur), `start`,
`step`. Il valide chaque action contre l'`action_space` (un agent n'est pas
de confiance) et redirige tout `print` d'un environnement vers stderr pour
ne pas corrompre le protocole. `wire.py` fixe le format des spaces et des
valeurs sur le fil — le SDK en implémente l'autre moitié : toute
modification doit y être reportée.

`.cargo/config.toml` ajoute des flags de lien macOS : sans eux, la feature
`extension-module` de PyO3 casse `cargo build/test --workspace` sur macOS.
Un simple `cargo build -p engine-python` ne produit pas un module
importable : toujours passer par `maturin develop`.

## `engine` — moteur du football

API façon Gym : `Engine::new(config, seed)`, puis `reset()` /
`step(actions)`. **Déterminisme total** exigé : même seed + mêmes actions =
même résultat, tick pour tick (rapier2d + RNG seedé, jamais de
`rand::thread_rng()` dans la logique de simulation elle-même).

Décisions à connaître avant de toucher à `engine/src/sim.rs` :
- **1 agent = 1 équipe** : `Actions` transporte les actions de TOUS les
  joueurs d'une équipe.
- **Repère miroir, pour les observations ET les actions** : chaque équipe
  voit `self_team` comme la sienne, attaque vers `x = +1`, et ses actions
  sont exprimées dans ce même repère (`normalize` pour les observations,
  `to_field_frame` pour les actions). Bug réel corrigé : l'équipe 1 avait des
  observations mirrorées mais pas ses actions, et marquait contre son camp
  (trouvé en faisant jouer la même politique aux deux équipes en réseau :
  113-0 ; cf. `tests/action_frame.rs`).
- **Pas de murs sur les lignes de but** : un but est détecté "à la main"
  après chaque pas de physique (`resolve_ball_bounds`).
- **Formation de coup d'envoi** : le joueur n°0 de chaque équipe démarre
  juste à côté du ballon, face à l'adversaire. Un tir immédiat, ou deux
  joueurs qui avancent tout droit, se percutent — les tests dégagent d'abord
  le couloir (cf. `goal_detection.rs`, `action_frame.rs`).

## `protocol` — messages réseau

`Hello/Welcome/AuthError/MatchStart/ActionMessage/ObservationMessage/MatchEnd/Ping/Pong`.
Observations, actions, spaces et infos sont des `rmpv::Value` opaques : leur
forme dépend de l'environnement, qui les décrit (spaces envoyés dans
`MatchStart`) et les valide. Chaque `ObservationMessage` porte la récompense
et `last_action` (`Applied`/`Rejected`/`Missing`) : sans ce retour, un agent
dont les actions sont refusées jouerait l'action neutre sans le savoir.
`MatchEnd` donne les retours cumulés de tous les agents + les forfaits.

**Format MessagePack (vérifié contre un décodeur Python — ne pas changer sans
mettre à jour le SDK) :** structs = maps `{"champ": valeur}`
(`rmp_serde::to_vec_named`) ; enums à données = maps à une clé
`{"Welcome": {...}}` ; enums SANS données (`ActionStatus`) = la chaîne du
nom de la variante (`"Rejected"`).

## `server` — gateway + match runner

Package Cargo avec un **lib + deux bins** (le lib permet aux tests
d'intégration de démarrer un vrai serveur en mémoire) :
- `frondori-server` (`src/main.rs`) : le serveur lui-même. Décrit les
  environnements au démarrage via un worker (fatal si impossible).
- `manage-tokens` (`src/bin/manage_tokens.rs`) : CLI des tokens
  (`add <token> <player_id> <nom>` / `list` / `remove`).

**`environments`** (`src/environments.rs`) : catalogue + pilotage d'un
worker (`EnvWorker`, `kill_on_drop` : jamais de worker orphelin). Commande
configurable via `FRONDORI_ENV_WORKER` (défaut `python3 -m
frondori_engine.worker` ; en dev, le python du venv d'`engine-python`).

**`gateway`** (`src/gateway/`) : handshake + matchmaking FIFO. **Le client
choisit l'environnement à chaque connexion** (`Hello.environment`) : un
token identifie un agent (un modèle), qui peut jouer à tous les
environnements — y compris plusieurs en même temps. Le classement est tenu
par couple (agent, environnement), côté site. Une file par environnement,
jusqu'à avoir autant de participants que l'environnement a d'agents. Files protégées par `std::sync::Mutex` (aucun `.await` verrou
tenu) ; ping de chaque joueur en attente avant de lancer un match.

**`match_runner`** (`src/match_runner/`) : boucle à la cadence de
l'environnement (`MatchRunnerConfig.tick_rate_override` pour les tests et
outils de dev), délai d'action = max(période du tick, 50 ms). Action absente
ou invalide -> action neutre (côté worker). **Une action est rattachée à son
`tick`** : seule celle qui répond à la dernière observation est acceptée,
une action en retard est jetée. Bug réel corrigé : le serveur prenait le
prochain message du socket sans regarder son tick, et après un seul retard
l'agent jouait avec un pas de décalage jusqu'à la fin du match (trouvé avec
une politique de cuisine scriptée : 13 soupes en local, 7 en réseau ; cf.
`tests/late_action_test.rs`). Déconnexion d'un agent encore en
jeu -> fin immédiate, forfait, pas de reconnexion. Panne de l'environnement
-> statut `aborted` (le résultat ne doit pas compter). **Piège connu** : le
serveur ferme la connexion juste après `MatchEnd` — un client doit tolérer
un échec de fermeture de sa propre connexion (cf. `Agent.play` du SDK).

**`auth`** (`src/auth/`) : trait `AuthProvider` -> `PlayerId`.
`InMemoryAuthProvider` (tests, dev) ou `PostgresAuthProvider` (si
`DATABASE_URL`, table `players (token, player_id, display_name, created_at)`).
Erreur de connexion à Postgres au démarrage = **fatale** (volontaire).

**Persistance** (`src/matches/`) : tables `matches (id, environment, status,
replay, ...)` et `match_participants (match_id, seat, agent, player_id,
final_return, forfeited, final_info)` — un match peut avoir N agents. Le
replay est la liste des scènes JSON (une par pas). Tables créées/possédées
par Rust (`CREATE TABLE IF NOT EXISTS`), lues par Laravel.
- `environments` : le catalogue, republié à chaque démarrage du serveur
  (`record_environments`) ; un environnement disparu du code passe
  `available = false` (jamais supprimé : ses matchs y font référence). Le
  site ne code AUCUN environnement en dur, il lit cette table. Les bornes
  infinies des spaces y sont écrites `"inf"`/`"-inf"` (JSON n'a pas
  d'infini ; `serde_json` aurait mis `null`, constaté).
- **Piège** : `CREATE TABLE IF NOT EXISTS` n'est pas sûr en concurrence
  (deux créations simultanées -> `duplicate key ... pg_type_typname_nsp_index`,
  constaté avec deux tests en parallèle). `ensure_schema` prend un verrou
  consultatif (`pg_advisory_xact_lock`) le temps de la création.

Outil de dev : `server/examples/fast_server.rs` — le vrai serveur à cadence
accélérée (`FRONDORI_TICK_RATE`, défaut 300), port 8081, tokens en mémoire
(`token-a`, `token-b`, valables pour tous les environnements) ou Postgres si
`DATABASE_URL`.

### Setup PostgreSQL local (dev)

Postgres 18 installé via Homebrew, base `frondori` créée. **Piège** :
`postgres:///frondori` (sans utilisateur) échoue avec `role "anonymous" does
not exist` — toujours mettre l'utilisateur dans l'URL :

```bash
export DATABASE_URL="postgres://melvine@localhost/frondori"
export FRONDORI_ENV_WORKER="$PWD/engine-python/.venv/bin/python -m frondori_engine.worker"
cargo run -p server --bin manage-tokens -- add <token> <player_id> "<nom>"
cargo run -p server --bin frondori-server
```

## Commandes utiles

```bash
# Prérequis des tests serveur : le venv d'engine-python construit (les tests
# jouent de VRAIS environnements via de vrais workers).
cd engine-python && python -m venv .venv && source .venv/bin/activate \
  && pip install maturin && maturin develop && cd ..

cargo test --workspace
DATABASE_URL="postgres://melvine@localhost/frondori" cargo test --workspace

cd engine-python && source .venv/bin/activate && python -m pytest
python examples/random_agent.py kitchen-v0
```

## État du projet / ce qui reste

Chantier multi-environnements (fait, phases 1 à 2c) : contrat PettingZoo,
registre versionné, rendu en scène, worker, protocole générique,
matchmaking par environnement, persistance multi-agents, SDK générique, et
`frondori-web` sur ce schéma — catalogue lu dans `environments`, classement
par couple (agent, environnement) dans `agent_ratings` (commande
`matches:update-ratings` : ELO en duel, retour moyen sinon ; un forfait
annule un match coopératif, un match `aborted` ne compte jamais), rendu
générique des scènes (`resources/js/lib/scene.js`), archives de match
(`match.json` avec les spaces + `replay.json`). Validé de bout en bout :
vrais clients SDK -> `fast_server` + Postgres -> classements -> pages.

Hors scope pour l'instant : règles avancées du football (hors-jeu, fautes),
reconnexion en cours de match, CI.

## Convention de travail sur ce projet

L'utilisateur découvre Rust (à l'aise en C/Python). Commenter le code Rust
de façon appuyée : expliquer le POURQUOI des choix (ownership, async,
traits...) autant que le code lui-même. Avant de déclarer une fonctionnalité
réseau "terminée", valider avec un vrai test de bout en bout (vrais
sockets/vrais processus), pas seulement `cargo check`/tests unitaires —
plusieurs bugs réels de ce projet (race de fin de match, format MessagePack,
collision de coup d'envoi, course de fermeture côté SDK, actions non
mirrorées de l'équipe 1) n'ont été trouvés qu'en conditions réelles.
