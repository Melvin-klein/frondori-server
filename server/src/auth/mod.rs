//! Abstraction d'authentification. `AuthProvider` est le seul point de
//! contact entre le gateway et la source de vérité des tokens : le gateway
//! ne sait pas si elle vient d'une `HashMap` en mémoire ou d'une vraie base
//! Postgres (cf. `postgres.rs`).

pub mod postgres;

use async_trait::async_trait;
use std::collections::HashMap;

pub use postgres::PostgresAuthProvider;

/// Identifiant stable d'un participant authentifié. `String` pour l'instant ;
/// pourrait devenir un type dédié (`struct PlayerId(Uuid)`) si besoin de plus
/// de garanties de type plus tard.
pub type PlayerId = String;

/// Erreur retournée quand un token est invalide/expiré, ou quand la
/// vérification elle-même a échoué (ex: base de données injoignable — dans
/// ce cas aussi, on refuse la connexion plutôt que de risquer un faux
/// positif).
#[derive(Debug, Clone)]
pub struct AuthError {
    pub reason: String,
}

/// Abstraction sur la vérification des tokens des participants.
///
/// `Send + Sync` : contraintes nécessaires pour qu'une valeur `Arc<dyn
/// AuthProvider>` puisse être partagée entre les tâches tokio de chaque
/// connexion (chaque connexion tourne dans sa propre tâche asynchrone,
/// potentiellement sur des threads différents du pool tokio).
#[async_trait]
pub trait AuthProvider: Send + Sync {
    async fn authenticate(&self, token: &str) -> Result<PlayerId, AuthError>;
}

/// Implémentation en mémoire : une table `token -> PlayerId` figée au
/// démarrage. Utile pour les tests (cf. `tests/smoke_test.rs`) et le
/// développement local rapide, sans dépendre d'une base de données — mais
/// AUCUN token ajouté ici ne survit à un redémarrage du process, cf.
/// `PostgresAuthProvider` pour un usage réel.
pub struct InMemoryAuthProvider {
    tokens: HashMap<String, PlayerId>,
}

impl InMemoryAuthProvider {
    pub fn new(tokens: HashMap<String, PlayerId>) -> Self {
        Self { tokens }
    }
}

#[async_trait]
impl AuthProvider for InMemoryAuthProvider {
    // Pas de véritable opération asynchrone ici (juste une lecture de
    // `HashMap` en mémoire), mais la signature reste `async` pour respecter
    // le trait `AuthProvider` : `PostgresAuthProvider`, elle, fait un vrai
    // appel réseau `.await` à cet endroit précis.
    async fn authenticate(&self, token: &str) -> Result<PlayerId, AuthError> {
        self.tokens.get(token).cloned().ok_or_else(|| AuthError {
            reason: "token inconnu".to_string(),
        })
    }
}
