# Frondori — plateforme de compétition et de recherche en IA

Plateforme où des participants (étudiants, chercheurs, développeurs) hébergent
leur propre modèle chez eux et se connectent via un SDK, en WebSocket, pour
le faire jouer dans des environnements de simulation. Le premier est un
football 2D ; d'autres, sans aucun rapport, vont s'ajouter au fil des
questions de recherche. Objectif : performance temps réel + architecture
simple à maintenir, et ajouter un environnement sans toucher au serveur.

## Vue d'ensemble

Plusieurs dépôts, en dossiers frères (`~/Code/Frondori/`) :

```
frondori-server/     CE dépôt : protocole + serveur (gateway, match runner), AUCUN jeu
├── protocol/        messages serveur <-> SDK, encodage MessagePack
├── server/          gateway (auth + matchmaking) + match runner
├── scripts/         setup-environments.sh : le Python du worker (.venv)
└── viewer/          visionneuse spectateur locale (HTML statique, tout environnement)
frondori-engine/     le contrat commun des environnements (registre, scènes, spaces, worker, tests du contrat)
frondori-football/   football-v0 : moteur Rust (crate engine) + bindings PyO3 + environnement
frondori-kitchen/    kitchen-v0 : cuisine coopérative, Python pur
frondori-sdk-python/ SDK des participants (en ligne, ou local=True)
frondori-web/        le site (Laravel, même base Postgres)
```

**Chaque environnement est un paquet Python indépendant**, dans son propre
dépôt, déclaré par un *entry point* (groupe `frondori.environments`) et
découvert automatiquement par `frondori-engine`. Un chercheur n'installe que
ceux qui l'intéressent ; le contrat et ses pièges sont documentés dans le
`CLAUDE.md` de `frondori-engine`, le moteur du football dans celui de
`frondori-football`.

**Le serveur ne contient aucune règle de jeu.** Chaque match lance un
worker Python (`python -m frondori_engine.worker`, un processus par match)
qui exécute l'environnement ; le serveur relaie observations et actions
entre les participants et ce worker. Le serveur propose exactement les
environnements installés dans le Python de ce worker (`.venv` de ce dépôt,
préparé par `scripts/setup-environments.sh` avec les dépôts frères en mode
éditable) : ajouter un jeu = l'installer là, redémarrer le serveur. Le
football (Rust) passe par le même chemin, via ses bindings PyO3 : ~0,2 ms par
pas. Le catalogue publié (table `environments`) indique le paquet de chaque
environnement (`package`), que le site affiche en commande `pip install`.

Sécurité : le code d'un environnement s'exécute sur le serveur ; l'isolation
par processus protège des plantages, pas d'un code malveillant — n'installer
que des paquets relus.

## `protocol` — messages réseau

`Hello/Welcome/AuthError/MatchStart/ActionMessage/ObservationMessage/MatchEnd/Ping/Pong`
(version 0.3 : `MatchStart.compute_budget_ms`, `ActionMessage.compute_ms`,
statut `TooSlow`).
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
frondori_engine.worker` ; en dev, le `.venv` de ce dépôt, cf. `scripts/`).

**`gateway`** (`src/gateway/`) : handshake + matchmaking FIFO. **Le client
choisit l'environnement à chaque connexion** (`Hello.environment`) : un
token identifie un agent (un modèle), qui peut jouer à tous les
environnements — y compris plusieurs en même temps. Le classement est tenu
par couple (agent, environnement), côté site. Une file par environnement,
jusqu'à avoir autant de participants que l'environnement a d'agents. Files protégées par `std::sync::Mutex` (aucun `.await` verrou
tenu) ; ping de chaque joueur en attente avant de lancer un match.

**`match_runner`** (`src/match_runner/`) : **pas-à-pas**. À chaque pas, le
serveur attend l'action de CHAQUE agent avant d'avancer (comme
`env.step(actions)` en local) : la latence réseau d'un participant ne lui
coûte rien, elle rallonge seulement le match. Ce qui est limité, c'est le
TEMPS DE CALCUL, mesuré par le client (de la réception de l'observation à
l'envoi de l'action) et déclaré avec chaque action (`compute_ms`) ; au-delà
du budget de l'environnement -> `TooSlow` + action neutre. La cadence de
l'environnement n'est qu'un plancher (`tokio::join!` des actions et du
ticker) ; `MatchRunnerConfig.response_timeout` (2 s) ne sert qu'à ne pas
bloquer sur un client planté (-> `Missing`). Action invalide -> action
neutre (côté worker).
- `timing.rs` : un temps déclaré n'est pas vérifiable (le client tourne chez
  le participant). Le serveur le confronte au temps de réponse qu'il mesure
  et à l'aller-retour réseau (`Ping` envoyés pendant le match, juste après
  que tous ont répondu) : un temps inexpliqué médian > 50 ms = `suspect`
  (loggé, affiché sur le site, mais le match compte). Côté site, un agent
  avec plus de 3 matchs suspects (tous environnements) porte un label
  "Timing inconsistencies" (`Agent::SUSPECT_MATCHES_THRESHOLD`) : il
  informe, il n'exclut rien du classement (choix de l'utilisateur). Dissuasion, pas
  preuve : un client modifié peut aussi retarder ses `Pong`. Bilan
  (`timing`) et détail pas par pas (`step_timings`) enregistrés par
  participant : ce sont aussi des données de recherche.
- **Piège trouvé** : en passant au pas-à-pas, un reste de l'ancienne boucle
  attendait le rythme une seconde fois avant d'écouter les actions — matchs
  deux fois trop longs et clients honnêtes déclarés suspects. Invisible à
  1000 pas/s : vu en réseau réel à 5 pas/s (cf. `lockstep_test.rs`, test de
  cadence). Valider le pas-à-pas à la VRAIE cadence. **Une action est rattachée à son
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
export FRONDORI_ENV_WORKER="$PWD/.venv/bin/python -m frondori_engine.worker"
cargo run -p server --bin manage-tokens -- add <token> <player_id> "<nom>"
cargo run -p server --bin frondori-server
```

## Commandes utiles

```bash
# Prérequis des tests serveur : le Python du worker, avec les environnements
# des dépôts frères (les tests jouent de VRAIS environnements via de vrais
# workers). Le football se compile : toolchain Rust requise.
scripts/setup-environments.sh

cargo test --workspace
DATABASE_URL="postgres://melvine@localhost/frondori" cargo test --workspace

# Chaque environnement, et frondori-engine, se testent dans leur dépôt :
cd ../frondori-kitchen && python -m pytest
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

Environnements sortis du serveur (fait) : un dépôt et un paquet par
environnement (`frondori-football`, `frondori-kitchen`), découverts par entry
points via `frondori-engine` ; le SDK joue aussi en local (`local=True`,
mêmes conditions qu'en compétition). Validé : un venv avec la seule cuisine
ne voit que `kitchen-v0` ; match local (13 soupes, comme en ligne) ; serveur
réel jouant les environnements externes ; paquet d'exemple créé en suivant
la documentation.

Adresses : le site sur `frondori.com` (`www` redirige), le serveur de jeu
sur son propre sous-domaine, `wss://play.frondori.com` (`/agent`,
`/spectate/:id`), derrière un reverse proxy qui termine le TLS. C'est
l'adresse par défaut du SDK (`DEFAULT_URL`, surchargeable par `FRONDORI_URL`)
: elle est figée dans chaque SDK installé, ne jamais la changer — déplacer le
serveur = changer le DNS de `play`. Proxy : délai d'inactivité de plusieurs
minutes (un agent peut attendre en file sans échanger de message ; nginx
coupe à 60 s par défaut). Dépôts GitHub : `github.com/Melvin-klein/<dépôt>`
(`frondori-web` privé).

Pas encore fait : publication sur PyPI (wheels précompilées du football :
CI multi-plateformes), déploiement.
La seed d'un match n'est pas enregistrée et les actions non plus : un match
n'est pas rejouable à l'identique.

Hors scope pour l'instant : règles avancées du football (hors-jeu, fautes),
reconnexion en cours de match, CI.

## Licences

MIT pour ce dépôt, `frondori-engine`, les environnements et le SDK.
`frondori-web` est propriétaire (tous droits réservés).

## Convention de travail sur ce projet

L'utilisateur découvre Rust (à l'aise en C/Python). Commenter le code Rust
de façon appuyée : expliquer le POURQUOI des choix (ownership, async,
traits...) autant que le code lui-même. Avant de déclarer une fonctionnalité
réseau "terminée", valider avec un vrai test de bout en bout (vrais
sockets/vrais processus), pas seulement `cargo check`/tests unitaires —
plusieurs bugs réels de ce projet (race de fin de match, format MessagePack,
collision de coup d'envoi, course de fermeture côté SDK, actions non
mirrorées de l'équipe 1) n'ont été trouvés qu'en conditions réelles.
