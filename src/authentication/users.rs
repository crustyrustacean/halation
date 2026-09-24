// src/authentication/users.rs

// dependencies
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// The live-account row. Not `Serialize` on purpose: there is no code path
/// where this type may leak into a template context or a response body.
/// Public-facing pages get a separate read-only view type (Phase 4).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// What successful registration produces — never holds raw passwords.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub username: String,
    pub email: String,
    pub password_hash: String,
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
