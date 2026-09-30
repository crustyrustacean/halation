// src/authentication/users.rs
// dependencies
use crate::authentication::password::PasswordHashSecret;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// The live-account row.
///
/// Not `Serialize` on purpose: there is no code path where this type may
/// leak into a template context or a response body. Public-facing pages get
/// a separate read-only view type (`ProfileView`).
///
/// `Debug` is written by hand for the same reason, plus one more: the
/// `password_hash` is a `PasswordHashSecret` whose own `Debug` redacts, so
/// even a `#[tracing::instrument]` on a future function that takes a
/// `User` records a placeholder rather than a replayable credential. The
/// omission is visible at the definition site rather than implied by a
/// derive.
#[derive(Clone, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub password_hash: PasswordHashSecret,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `password_hash` is deliberately absent, not merely redacted:
        // there is no diagnostic value in it, and every appearance in a
        // log is pure risk.
        f.debug_struct("User")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("email", &self.email)
            .field("display_name", &self.display_name)
            .field("bio", &self.bio)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("deleted_at", &self.deleted_at)
            .finish_non_exhaustive()
    }
}

/// What successful registration produces — never holds raw passwords.
#[derive(Clone)]
pub struct NewUser {
    pub username: String,
    pub email: String,
    pub password_hash: PasswordHashSecret,
}

impl std::fmt::Debug for NewUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewUser")
            .field("username", &self.username)
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UserStoreError {
    #[error("username already taken")]
    DuplicateUsername,

    #[error("email already registered")]
    DuplicateEmail,

    #[error(transparent)]
    Operation(#[from] anyhow::Error),
}
