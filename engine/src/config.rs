/// Paramètres de simulation, figés à la création d'un [`crate::Engine`].
///
/// Regrouper tous les paramètres dans une struct (plutôt que de les passer un
/// par un à `Engine::new`) permet de dériver `Clone`/`Debug` automatiquement
/// (utile pour logger la config d'un match), et de tout modifier à un seul
/// endroit si on ajoute un paramètre plus tard.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Nombre de joueurs par équipe (ex: 3 pour un match "3v3").
    /// C'est un `usize` au runtime, pas une constante de compilation : on
    /// peut donc lancer des matchs 3v3 et 5v5 avec le même code, juste avec
    /// une config différente.
    pub players_per_team: usize,

    /// Largeur du terrain en mètres, sur l'axe X (du but gauche au but droit).
    pub field_width: f32,
    /// Hauteur du terrain en mètres, sur l'axe Y.
    pub field_height: f32,

    /// Rayon d'un joueur (chaque joueur est un disque rigide rapier2d), en mètres.
    pub player_radius: f32,
    /// Rayon du ballon, en mètres.
    pub ball_radius: f32,

    /// Largeur de la cage, centrée sur `field_height / 2.0`, sur l'axe Y.
    /// Un but est marqué quand le ballon franchit x = 0 (ou x = field_width)
    /// avec une position Y comprise dans [center - goal_width/2, center + goal_width/2].
    pub goal_width: f32,

    /// Durée simulée d'un tick, en secondes (ex: 1.0 / 30.0 pour 30 Hz).
    /// Doit rester identique entre deux runs pour garantir le déterminisme
    /// (rapier2d intègre la physique avec un pas de temps fixe = ce `dt`).
    pub dt: f32,

    /// Nombre maximum de ticks avant fin de match automatique.
    pub max_ticks: u32,

    /// Score (buts d'une équipe) à partir duquel le match s'arrête
    /// immédiatement. `None` = pas de limite, seul `max_ticks` compte.
    pub max_score: Option<u32>,

    // --- Paramètres de gameplay, ajoutés lors de l'implémentation --------
    // (absents de la première version du skeleton, nécessaires pour piloter
    // la physique : vitesse des joueurs, frottement du ballon, portée de
    // tir...). Regroupés ici plutôt qu'en constantes privées dans `sim.rs`
    // pour rester modifiables sans recompiler la logique du moteur.
    /// Vitesse maximale d'un joueur, en mètres/seconde. Un `Action.move_dir`
    /// de norme 1.0 fait courir le joueur à cette vitesse.
    pub player_max_speed: f32,

    /// Amortissement linéaire du ballon (frottement), appliqué en continu
    /// par rapier2d (`RigidBody::set_linear_damping`) : plus la valeur est
    /// grande, plus le ballon ralentit vite une fois lancé.
    pub ball_linear_damping: f32,

    /// Coefficient de restitution (rebond) du ballon sur les joueurs et les
    /// limites de terrain. 0.0 = pas de rebond (le ballon s'arrête net),
    /// 1.0 = rebond parfait sans perte d'énergie.
    pub ball_restitution: f32,

    /// Distance additionnelle, au-delà du simple contact (rayon joueur +
    /// rayon ballon), à laquelle un joueur peut encore tirer/passer.
    pub kick_range: f32,

    /// Vitesse ajoutée au ballon (en m/s, dans la direction du tir) pour un
    /// `Action.kick` de norme 1.0 (puissance maximale).
    pub kick_max_speed: f32,

    /// Amplitude maximale (en mètres) de la variation aléatoire appliquée à
    /// la position des joueurs "de champ" (hors les deux joueurs au contact
    /// du ballon) à chaque coup d'envoi. Tirée depuis le générateur seedé de
    /// l'`Engine`, donc reproductible à seed égal.
    pub formation_jitter: f32,
}

// `impl Default for EngineConfig` permet d'écrire `EngineConfig::default()`
// ou `EngineConfig { players_per_team: 5, ..Default::default() }` pour ne
// changer que certains champs. C'est l'équivalent Rust idiomatique de valeurs
// par défaut de paramètres, qui n'existent pas nativement pour les structs.
impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            players_per_team: 3,
            field_width: 40.0,
            field_height: 20.0,
            player_radius: 0.5,
            ball_radius: 0.2,
            goal_width: 6.0,
            dt: 1.0 / 30.0,
            max_ticks: 30 * 60 * 5, // 5 minutes à 30 Hz
            max_score: None,
            player_max_speed: 6.0,
            ball_linear_damping: 0.6,
            ball_restitution: 0.6,
            kick_range: 0.4,
            kick_max_speed: 12.0,
            formation_jitter: 0.5,
        }
    }
}
