//! Le moteur lui-même : encapsule l'état physique rapier2d et expose l'API
//! façon Gym (`reset`/`step`).
//!
//! `rapier2d::prelude::*` importe d'un coup tous les types courants du
//! moteur physique (builders, sets, pipeline...) : c'est le module prévu par
//! la crate exactement pour cet usage, plutôt que d'importer chaque type un
//! par un depuis `rapier2d::dynamics::...`.
use rapier2d::prelude::*;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::config::EngineConfig;
use crate::types::{
    Actions, BallObs, Info, Observation, PlayerObs, Reward, SpectatorFrame, SpectatorPlayer,
    StepResult,
};

/// Épaisseur des murs statiques (lignes de touche haut/bas), en mètres.
/// Juste un détail d'implémentation interne, pas besoin de l'exposer dans
/// `EngineConfig`.
const WALL_THICKNESS: f32 = 1.0;

/// Le moteur de simulation.
///
/// Pas de dépendance réseau ici : `Engine` est utilisable comme une simple
/// librairie de simulation (voir `examples/random_agent.rs`).
pub struct Engine {
    config: EngineConfig,

    /// Générateur pseudo-aléatoire *seedé* : deux `Engine` créés avec le
    /// même `seed` tirent exactement la même séquence de nombres, dans le
    /// même ordre, ce qui est la condition du déterminisme total dès qu'on
    /// utilise `rng` (ici : la variation aléatoire de formation au coup
    /// d'envoi, cf. `reset_positions`).
    rng: StdRng,

    // --- État rapier2d ---------------------------------------------------
    // Ce sont les structures « bas niveau » du moteur physique : la liste
    // des corps rigides, des collideurs (formes physiques attachées aux
    // corps), et tous les sous-systèmes que `PhysicsPipeline::step` doit
    // recevoir pour avancer la simulation d'un pas de temps. On ne les
    // manipule jamais directement depuis l'extérieur du crate (tous ces
    // champs sont privés) : rapier2d est un détail d'implémentation.
    rigid_body_set: RigidBodySet,
    collider_set: ColliderSet,
    integration_parameters: IntegrationParameters,
    physics_pipeline: PhysicsPipeline,
    island_manager: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    impulse_joint_set: ImpulseJointSet,
    multibody_joint_set: MultibodyJointSet,
    ccd_solver: CCDSolver,

    // --- État "métier" -----------------------------------------------------
    /// `player_bodies[0]` = handles des joueurs de l'équipe 0 (défend x=0,
    /// attaque x=field_width), `player_bodies[1]` = équipe 1 (symétrique).
    /// Un `RigidBodyHandle` est un identifiant léger (Copy), pas un
    /// pointeur : il reste valide tant qu'on ne retire pas le corps du
    /// `RigidBodySet` (ce qu'on ne fait jamais ici, les corps sont créés une
    /// fois dans `new` et juste repositionnés ensuite).
    player_bodies: [Vec<RigidBodyHandle>; 2],
    ball_body: RigidBodyHandle,

    tick: u32,
    /// `(buts de l'équipe 0, buts de l'équipe 1)`, convention interne au
    /// moteur (non exposée telle quelle : `Observation::score` est déjà
    /// réordonné en `(soi, adversaire)` par `build_observation`).
    score: (u32, u32),
}

impl Engine {
    /// Crée un nouveau moteur pour un match : construit le terrain, les
    /// joueurs et le ballon (une seule fois), puis les place en position de
    /// coup d'envoi.
    pub fn new(config: EngineConfig, seed: u64) -> Self {
        let mut rigid_body_set = RigidBodySet::new();
        let mut collider_set = ColliderSet::new();

        let half_w = config.field_width / 2.0;

        // Lignes de touche (haut/bas) : deux murs statiques avec rebond.
        // Volontairement plus larges que le terrain (`+ WALL_THICKNESS` de
        // chaque côté) pour bien couvrir les coins.
        //
        // Aucun mur sur les lignes de but (gauche/droite) : le ballon doit
        // pouvoir les franchir librement. La distinction "but marqué" vs
        // "sortie de terrain simple" est faite nous-mêmes après chaque pas
        // de simulation, dans `resolve_ball_bounds`.
        for wall_center_y in [
            config.field_height + WALL_THICKNESS / 2.0,
            -WALL_THICKNESS / 2.0,
        ] {
            collider_set.insert(
                ColliderBuilder::cuboid(half_w + WALL_THICKNESS, WALL_THICKNESS / 2.0)
                    .translation(vector![half_w, wall_center_y])
                    .friction(0.0)
                    .restitution(0.8)
                    .build(),
            );
        }

        // `[Vec::new(), Vec::new()]` : un vecteur de handles par équipe.
        let mut player_bodies: [Vec<RigidBodyHandle>; 2] = [Vec::new(), Vec::new()];
        for team_players in player_bodies.iter_mut() {
            for _ in 0..config.players_per_team {
                // `.lock_rotations()` : un joueur est un disque, sa
                // rotation propre n'a aucun sens dans ce jeu (elle
                // n'apparaît nulle part dans `Observation`) ; la bloquer
                // évite des calculs inutiles et des comportements de
                // "toupie" instables lors des contacts.
                let body = RigidBodyBuilder::dynamic().lock_rotations().build();
                let handle = rigid_body_set.insert(body);

                let collider = ColliderBuilder::ball(config.player_radius)
                    .friction(0.5)
                    .restitution(0.0) // les joueurs se bloquent entre eux, ils ne rebondissent pas
                    .build();
                collider_set.insert_with_parent(collider, handle, &mut rigid_body_set);

                team_players.push(handle);
            }
        }

        let ball_rigid_body = RigidBodyBuilder::dynamic()
            .lock_rotations()
            .linear_damping(config.ball_linear_damping) // simule le frottement du ballon sur le sol
            .ccd_enabled(true) // évite qu'un tir rapide "traverse" un joueur sans collision détectée
            .build();
        let ball_body = rigid_body_set.insert(ball_rigid_body);
        let ball_collider = ColliderBuilder::ball(config.ball_radius)
            .friction(0.3)
            .restitution(config.ball_restitution)
            .build();
        collider_set.insert_with_parent(ball_collider, ball_body, &mut rigid_body_set);

        let mut integration_parameters = IntegrationParameters::default();
        integration_parameters.dt = config.dt;

        let mut engine = Self {
            rng: StdRng::seed_from_u64(seed),
            rigid_body_set,
            collider_set,
            integration_parameters,
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            player_bodies,
            ball_body,
            tick: 0,
            score: (0, 0),
            config,
        };

        // Place tout le monde en position de coup d'envoi dès la création,
        // pour qu'un `Engine` fraîchement construit soit déjà dans un état
        // valide même si `reset()` n'a pas encore été appelé explicitement.
        engine.reset_positions();
        engine
    }

    /// Réinitialise le match : score à zéro, tick à zéro, joueurs et ballon
    /// replacés en position de coup d'envoi. Retourne l'observation initiale
    /// pour chaque équipe (`[0]` = équipe 0, `[1]` = équipe 1).
    pub fn reset(&mut self) -> [Observation; 2] {
        self.tick = 0;
        self.score = (0, 0);
        self.reset_positions();
        [self.build_observation(0), self.build_observation(1)]
    }

    /// Avance la simulation d'UN tick : applique les actions des deux
    /// équipes, fait avancer la physique de `config.dt` secondes, détecte
    /// les buts/sorties de terrain, et construit le résultat pour chaque
    /// équipe.
    pub fn step(&mut self, actions: [Actions; 2]) -> StepResult {
        self.apply_movement(&actions);
        self.apply_kicks(&actions);

        // `PhysicsPipeline::step` est la fonction centrale de rapier2d :
        // elle prend TOUS les sous-systèmes en paramètres (plutôt que de les
        // stocker elle-même) et avance l'état de `integration_parameters.dt`
        // secondes. `&vector![0.0, 0.0]` = pas de gravité (vue du dessus,
        // pas un jeu de plateforme). Les deux derniers arguments (`&()`,
        // `&()`) sont des "hooks" et un gestionnaire d'événements de
        // collision optionnels : on n'en a pas besoin en V1, et rapier2d
        // fournit justement une implémentation par défaut pour `()`.
        self.physics_pipeline.step(
            &vector![0.0, 0.0],
            &self.integration_parameters,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.rigid_body_set,
            &mut self.collider_set,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            &mut self.ccd_solver,
            None,
            &(),
            &(),
        );

        self.clamp_players_horizontally();
        let scoring_team = self.resolve_ball_bounds();

        self.tick += 1;

        let score_limit_reached = self
            .config
            .max_score
            .is_some_and(|max| self.score.0 >= max || self.score.1 >= max);
        let done = self.tick >= self.config.max_ticks || score_limit_reached;

        StepResult {
            observations: [self.build_observation(0), self.build_observation(1)],
            rewards: [
                Self::reward_for(0, scoring_team),
                Self::reward_for(1, scoring_team),
            ],
            done,
            info: Info { tick: self.tick },
        }
    }

    /// Construit l'état courant du match dans le référentiel RÉEL du
    /// terrain (jamais mirroré), pensé pour l'affichage (diffusion live aux
    /// spectateurs, enregistrement des replays) — jamais pour un agent, cf.
    /// `build_observation`/`Observation`.
    ///
    /// Contrairement à `step()`, ne fait AVANCER aucune simulation : elle ne
    /// fait que lire l'état actuel, donc utilisable à tout moment après
    /// `new`/`reset` (y compris juste après un `step`, pour diffuser l'état
    /// qui vient d'être calculé).
    pub fn spectator_frame(&self) -> SpectatorFrame {
        let half_w = self.config.field_width / 2.0;
        let half_h = self.config.field_height / 2.0;
        let center = vector![half_w, half_h];

        let mut players = Vec::with_capacity(2 * self.config.players_per_team);
        for (team, handles) in self.player_bodies.iter().enumerate() {
            for &handle in handles {
                let body = &self.rigid_body_set[handle];
                players.push(SpectatorPlayer {
                    team: team as u8,
                    // `mirror = false` : c'est précisément la différence
                    // avec `build_observation`, qui mirror pour l'équipe 1.
                    // Ici, on veut toujours le vrai terrain.
                    position: normalize(*body.translation() - center, half_w, half_h, false),
                    velocity: normalize(*body.linvel(), half_w, half_h, false),
                });
            }
        }

        let ball_rb = &self.rigid_body_set[self.ball_body];
        let ball = BallObs {
            position: normalize(*ball_rb.translation() - center, half_w, half_h, false),
            velocity: normalize(*ball_rb.linvel(), half_w, half_h, false),
        };

        SpectatorFrame {
            tick: self.tick,
            players,
            ball,
            score: self.score,
        }
    }

    // -------------------------------------------------------------------
    // Détails internes (privés)
    // -------------------------------------------------------------------

    /// Replace tous les corps (joueurs + ballon) en position de coup
    /// d'envoi, vitesses remises à zéro. N'affecte ni le score ni le tick
    /// (c'est `reset()` qui s'en charge, `new()` s'en fiche puisqu'ils
    /// démarrent déjà à zéro).
    fn reset_positions(&mut self) {
        let n = self.config.players_per_team;
        for team in 0..2 {
            for index in 0..n {
                let (x, mut y) = Self::formation_slot(&self.config, team, index);

                // Petite variation aléatoire de la position en Y pour les
                // joueurs "de champ" (pas les deux joueurs au contact direct
                // du ballon au coup d'envoi, pour garantir qu'ils restent
                // toujours à portée de tir). Tirée de `self.rng` : à seed
                // égal, deux matchs auront exactement la même formation.
                if index > 0 {
                    let jitter = self.config.formation_jitter;
                    y += self.rng.gen_range(-jitter..=jitter);
                    y = y.clamp(
                        self.config.player_radius,
                        self.config.field_height - self.config.player_radius,
                    );
                }

                let handle = self.player_bodies[team][index];
                let body = &mut self.rigid_body_set[handle];
                body.set_translation(vector![x, y], true);
                body.set_linvel(vector![0.0, 0.0], true);
            }
        }

        let center = vector![self.config.field_width / 2.0, self.config.field_height / 2.0];
        let ball = &mut self.rigid_body_set[self.ball_body];
        ball.set_translation(center, true);
        ball.set_linvel(vector![0.0, 0.0], true);
    }

    /// Position de base (sans variation aléatoire) du joueur `index` de
    /// l'équipe `team` en formation de coup d'envoi.
    fn formation_slot(config: &EngineConfig, team: usize, index: usize) -> (f32, f32) {
        let w = config.field_width;
        let h = config.field_height;

        if index == 0 {
            // Le joueur n°0 de chaque équipe démarre juste à côté du
            // ballon (au centre du terrain), à une distance calculée pour
            // être TOUJOURS à portée de tir (`kick_range`) dès le premier
            // tick, quelle que soit la config, sans pour autant se
            // superposer au ballon.
            let kickoff_offset =
                config.player_radius + config.ball_radius + config.kick_range * 0.5;
            let x = if team == 0 {
                w / 2.0 - kickoff_offset
            } else {
                w / 2.0 + kickoff_offset
            };
            (x, h / 2.0)
        } else {
            // Les autres joueurs se répartissent en ligne sur leur moitié
            // de terrain (équipe 0 = quart gauche, équipe 1 = quart droit).
            let x = if team == 0 { w * 0.25 } else { w * 0.75 };
            let y = h * (index as f32) / (config.players_per_team as f32);
            (x, y.clamp(config.player_radius, h - config.player_radius))
        }
    }

    /// Applique le déplacement demandé par chaque action : on pilote
    /// directement la vitesse du joueur (pas de force/accélération), ce qui
    /// est simple et parfaitement déterministe. `move_dir` est traité comme
    /// une direction (normalisée) modulée par sa propre norme, plafonnée à
    /// 1.0 : un vecteur de norme 0.5 fait donc courir le joueur à moitié
    /// vitesse, un vecteur de norme >= 1.0 fait courir à vitesse maximale.
    fn apply_movement(&mut self, actions: &[Actions; 2]) {
        for team in 0..2 {
            for (&handle, action) in self.player_bodies[team]
                .iter()
                .zip(actions[team].players.iter())
            {
                let dir = to_field_frame(team, action.move_dir);
                let norm = dir.norm();
                let velocity = if norm > 0.0 {
                    (dir / norm) * norm.min(1.0) * self.config.player_max_speed
                } else {
                    vector![0.0, 0.0]
                };
                self.rigid_body_set[handle].set_linvel(velocity, true);
            }
        }
    }

    /// Applique les tirs/passes : pour chaque action avec `kick = Some(_)`,
    /// si le ballon est à portée du joueur, ajoute de la vitesse au ballon
    /// dans la direction demandée (la norme du vecteur encode la
    /// puissance, plafonnée à 1.0 = puissance maximale).
    ///
    /// Si plusieurs joueurs sont à portée le même tick, les tirs
    /// s'appliquent dans l'ordre équipe 0 puis équipe 1 (et par index de
    /// joueur au sein d'une équipe) : c'est arbitraire, mais déterministe,
    /// ce qui est ce qui compte pour le contrat de reproductibilité.
    fn apply_kicks(&mut self, actions: &[Actions; 2]) {
        let reach = self.config.player_radius + self.config.ball_radius + self.config.kick_range;

        for team in 0..2 {
            for (&handle, action) in self.player_bodies[team]
                .iter()
                .zip(actions[team].players.iter())
            {
                let Some(kick) = action.kick else {
                    continue;
                };
                let kick_dir = to_field_frame(team, kick);
                let power = kick_dir.norm();
                if power <= 0.0 {
                    continue;
                }

                let player_pos = *self.rigid_body_set[handle].translation();
                let ball_pos = *self.rigid_body_set[self.ball_body].translation();
                if (ball_pos - player_pos).norm() > reach {
                    continue; // ballon hors de portée : ce tir n'a aucun effet
                }

                let added_velocity =
                    (kick_dir / power) * power.min(1.0) * self.config.kick_max_speed;
                let ball = &mut self.rigid_body_set[self.ball_body];
                let new_velocity = ball.linvel() + added_velocity;
                ball.set_linvel(new_velocity, true);
            }
        }
    }

    /// Empêche les joueurs de sortir du terrain par les lignes de but
    /// (aucun mur physique là-bas, cf. `new`) : on les repousse manuellement
    /// s'ils dépassent `[0, field_width]` en X. Seul le ballon a le droit de
    /// sortir par ces lignes (`resolve_ball_bounds` gère son cas séparément).
    fn clamp_players_horizontally(&mut self) {
        let w = self.config.field_width;
        let r = self.config.player_radius;

        for team in 0..2 {
            for &handle in &self.player_bodies[team] {
                let body = &mut self.rigid_body_set[handle];
                let pos = *body.translation();
                let vel = *body.linvel();

                if pos.x < r {
                    body.set_translation(vector![r, pos.y], true);
                    body.set_linvel(vector![vel.x.max(0.0), vel.y], true);
                } else if pos.x > w - r {
                    body.set_translation(vector![w - r, pos.y], true);
                    body.set_linvel(vector![vel.x.min(0.0), vel.y], true);
                }
            }
        }
    }

    /// Vérifie si le ballon a franchi une ligne de but (à gauche ou à
    /// droite du terrain) et distingue deux cas :
    /// - dans le couloir de la cage (`|y - centre| <= goal_width/2`) : but
    ///   marqué, le score est incrémenté et tout le monde est repositionné
    ///   au coup d'envoi (`reset_positions`) ;
    /// - en dehors (mais toujours au-delà de la ligne) : simple sortie de
    ///   terrain, le ballon est juste replacé sur la ligne, vitesse nulle
    ///   ("remise en jeu simple", pas de touche/corner avec règle d'équipe
    ///   en V1).
    ///
    /// Retourne l'équipe qui a marqué (`Some(0)` ou `Some(1)`), ou `None`.
    fn resolve_ball_bounds(&mut self) -> Option<usize> {
        let w = self.config.field_width;
        let center_y = self.config.field_height / 2.0;
        let goal_half = self.config.goal_width / 2.0;

        let pos = *self.rigid_body_set[self.ball_body].translation();

        if pos.x < 0.0 {
            if (pos.y - center_y).abs() <= goal_half {
                self.score.1 += 1;
                self.reset_positions();
                return Some(1);
            }
            let ball = &mut self.rigid_body_set[self.ball_body];
            ball.set_translation(vector![0.0, pos.y], true);
            ball.set_linvel(vector![0.0, 0.0], true);
        } else if pos.x > w {
            if (pos.y - center_y).abs() <= goal_half {
                self.score.0 += 1;
                self.reset_positions();
                return Some(0);
            }
            let ball = &mut self.rigid_body_set[self.ball_body];
            ball.set_translation(vector![w, pos.y], true);
            ball.set_linvel(vector![0.0, 0.0], true);
        }

        None
    }

    /// Construit la `Reward` de l'équipe `team` à partir de l'équipe ayant
    /// marqué ce tick (`scoring_team`, `None` si aucun but). Fonction
    /// associée (pas de `&self`) : elle ne dépend que de ses paramètres, pas
    /// de l'état du moteur.
    fn reward_for(team: usize, scoring_team: Option<usize>) -> Reward {
        let opponent = 1 - team;
        Reward {
            goal_scored: if scoring_team == Some(team) { 1.0 } else { 0.0 },
            goal_conceded: if scoring_team == Some(opponent) {
                -1.0
            } else {
                0.0
            },
        }
    }

    /// Construit l'`Observation` de l'équipe `team`, dans SON référentiel :
    /// coordonnées normalisées dans `[-1, 1]`, miroir en X pour l'équipe 1
    /// (cf. doc d'`Observation` dans `types.rs` pour le pourquoi).
    fn build_observation(&self, team: usize) -> Observation {
        let mirror = team == 1;
        let half_w = self.config.field_width / 2.0;
        let half_h = self.config.field_height / 2.0;
        let center = vector![half_w, half_h];

        // Closure locale : convertit un `RigidBodyHandle` en `PlayerObs`
        // normalisé/mirroré. Définie ici plutôt qu'en méthode séparée car
        // elle capture `mirror`/`half_w`/`half_h`/`center`, qui varient
        // selon l'équipe demandée.
        let player_obs = |handle: RigidBodyHandle| -> PlayerObs {
            let body = &self.rigid_body_set[handle];
            let relative_pos = *body.translation() - center;
            let vel = *body.linvel();
            PlayerObs {
                position: normalize(relative_pos, half_w, half_h, mirror),
                velocity: normalize(vel, half_w, half_h, mirror),
            }
        };

        let opponent = 1 - team;
        let self_team = self.player_bodies[team]
            .iter()
            .map(|&h| player_obs(h))
            .collect();
        let opponent_team = self.player_bodies[opponent]
            .iter()
            .map(|&h| player_obs(h))
            .collect();

        let ball_rb = &self.rigid_body_set[self.ball_body];
        let ball = BallObs {
            position: normalize(*ball_rb.translation() - center, half_w, half_h, mirror),
            velocity: normalize(*ball_rb.linvel(), half_w, half_h, mirror),
        };

        let score = if team == 0 {
            self.score
        } else {
            (self.score.1, self.score.0)
        };

        Observation {
            self_team,
            opponent_team,
            ball,
            score,
            ticks_remaining: self.config.max_ticks.saturating_sub(self.tick),
        }
    }
}

/// Normalise un vecteur `(x, y)` en mètres vers `[-1, 1]` (approximativement :
/// une position à exactement `half_w`/`half_h` du centre donne pile 1.0/-1.0,
/// une vitesse peut dépasser 1.0 si elle dépasse `half_w`/`half_h` mètres par
/// seconde — l'échelle est la même que pour les positions, par cohérence).
/// Si `mirror` est vrai (équipe 1), la composante X est inversée : c'est
/// exactement ce qui fait que l'équipe 1 "voit" le terrain comme si elle
/// attaquait vers +1, alors qu'elle attaque physiquement vers -1.
fn normalize(v: Vector<Real>, half_w: f32, half_h: f32, mirror: bool) -> (f32, f32) {
    let x = v.x / half_w;
    let y = v.y / half_h;
    if mirror {
        (-x, y)
    } else {
        (x, y)
    }
}

/// Le symétrique de `normalize` pour les ACTIONS : un vecteur d'action
/// (`move_dir`, `kick`) est exprimé dans le repère de l'équipe qui l'envoie
/// — celui de son observation —, et doit être ramené dans le repère réel du
/// terrain avant d'être appliqué. Pour l'équipe 1, +x veut dire "vers le
/// but adverse", c'est-à-dire -x sur le vrai terrain.
///
/// Sans cette conversion, l'équipe 1 voyait un terrain retourné mais
/// agissait sur le terrain réel : une politique "j'avance vers +x" fonçait
/// vers son propre but (bug réel, cf. `tests/action_frame.rs`).
fn to_field_frame(team: usize, (x, y): (f32, f32)) -> Vector<Real> {
    if team == 1 {
        vector![-x, y]
    } else {
        vector![x, y]
    }
}
