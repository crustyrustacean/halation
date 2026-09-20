// src/authentication/users.rs

// dependencies
use anyhow::anyhow;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
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

const USER_COLUMNS: &str =
    "id, username::text AS username, email::text AS email, password_hash, display_name, bio, \
     created_at, updated_at, deleted_at";

/// Insert a new live account. Duplicate handles/emails on live accounts map
/// to the typed variants (the caller turns them into 409s); everything else
/// is an operational error.
pub async fn insert_user(pool: &PgPool, new_user: &NewUser) -> Result<User, UserStoreError> {
    sqlx::query_as::<_, User>(
        &format!(
            "INSERT INTO users (username, email, password_hash)
             VALUES ($1, $2, $3)
             RETURNING {USER_COLUMNS}"
        ),
    )
    .bind(&new_user.username)
    .bind(&new_user.email)
    .bind(&new_user.password_hash)
    .fetch_one(pool)
    .await
    .map_err(|e| match e.as_database_error().and_then(|db| db.constraint().map(String::from)) {
        Some(constraint) if constraint == "users_username_live_idx" => UserStoreError::DuplicateUsername,
        Some(constraint) if constraint == "users_email_live_idx" => UserStoreError::DuplicateEmail,
        _ => UserStoreError::Operation(anyhow!("Failed to insert user: {e}")),
    })
}

/// Find a live account by username or email (case-insensitive via CITEXT).
/// Deleted accounts are invisible to the application.
pub async fn find_by_identifier(
    pool: &PgPool,
    identifier: &str,
) -> Result<Option<User>, anyhow::Error> {
    let user = sqlx::query_as::<_, User>(
        &format!(
            "SELECT {USER_COLUMNS}
             FROM users
             WHERE (username = $1 OR email = $1) AND deleted_at IS NULL
             LIMIT 1"
        ),
    )
    .bind(identifier)
    .fetch_optional(pool)
    .await
    .map_err(|e| anyhow!("Failed to fetch user by identifier: {e}"))?;

    Ok(user)
}
