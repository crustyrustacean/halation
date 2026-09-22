// src/follows.rs
//!
//! The social graph: who follows whom, and the read helpers the profile
//! and list pages need.

use anyhow::anyhow;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// Follow `followee` as `follower`. Idempotent — a duplicate is a no-op.
pub async fn follow(pool: &PgPool, follower_id: Uuid, followee_id: Uuid) -> Result<(), anyhow::Error> {
    sqlx::query(
        "INSERT INTO follows (follower_id, followee_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(follower_id)
    .bind(followee_id)
    .execute(pool)
    .await
    .map_err(|e| anyhow!("Failed to follow: {e}"))?;
    Ok(())
}

/// Unfollow. Missing rows are a no-op (idempotent).
pub async fn unfollow(pool: &PgPool, follower_id: Uuid, followee_id: Uuid) -> Result<(), anyhow::Error> {
    sqlx::query("DELETE FROM follows WHERE follower_id = $1 AND followee_id = $2")
        .bind(follower_id)
        .bind(followee_id)
        .execute(pool)
        .await
        .map_err(|e| anyhow!("Failed to unfollow: {e}"))?;
    Ok(())
}

pub async fn is_following(pool: &PgPool, follower_id: Uuid, followee_id: Uuid) -> Result<bool, anyhow::Error> {
    let follows: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM follows WHERE follower_id = $1 AND followee_id = $2)",
    )
    .bind(follower_id)
    .bind(followee_id)
    .fetch_one(pool)
    .await
    .map_err(|e| anyhow!("Failed to check follow state: {e}"))?;
    Ok(follows)
}

pub async fn count_followers(pool: &PgPool, user_id: Uuid) -> Result<i64, anyhow::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM follows WHERE followee_id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|e| anyhow!("Failed to count followers: {e}"))
}

pub async fn count_following(pool: &PgPool, user_id: Uuid) -> Result<i64, anyhow::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM follows WHERE follower_id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|e| anyhow!("Failed to count following: {e}"))
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AccountSummary {
    pub username: String,
    pub display_name: String,
    /// First letter of the username, uppercased — the avatar initial.
    pub initial: String,
}

const ACCOUNT_COLUMNS: &str = "u.username::text AS username, COALESCE(u.display_name, u.username::text) AS display_name, UPPER(SUBSTRING(u.username FROM 1 FOR 1)) AS initial";

/// Accounts that follow `username`, newest follow first.
pub async fn followers_of(pool: &PgPool, username: &str) -> Result<Vec<AccountSummary>, anyhow::Error> {
    sqlx::query_as::<_, AccountSummary>(&format!(
        "SELECT {ACCOUNT_COLUMNS}
         FROM follows f
         JOIN users u ON u.id = f.follower_id
         JOIN users me ON me.id = f.followee_id
         WHERE me.username = $1
         ORDER BY f.created_at DESC, u.username"
    ))
    .bind(username)
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow!("Failed to load followers: {e}"))
    .map(|rows| {
        rows.into_iter()
            .map(|mut account| {
                account.initial = account
                    .username
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default();
                account
            })
            .collect()
    })
}

/// Accounts that `username` follows, newest follow first.
pub async fn following_of(pool: &PgPool, username: &str) -> Result<Vec<AccountSummary>, anyhow::Error> {
    sqlx::query_as::<_, AccountSummary>(&format!(
        "SELECT {ACCOUNT_COLUMNS}
         FROM follows f
         JOIN users u ON u.id = f.followee_id
         JOIN users me ON me.id = f.follower_id
         WHERE me.username = $1
         ORDER BY f.created_at DESC, u.username"
    ))
    .bind(username)
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow!("Failed to load following: {e}"))
    .map(|rows| {
        rows.into_iter()
            .map(|mut account| {
                account.initial = account
                    .username
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default();
                account
            })
            .collect()
    })
}

pub async fn find_user_id_by_username(pool: &PgPool, username: &str) -> Result<Option<Uuid>, anyhow::Error> {
    sqlx::query_scalar("SELECT id FROM users WHERE username = $1 AND deleted_at IS NULL")
        .bind(username)
        .fetch_optional(pool)
        .await
        .map_err(|e| anyhow!("Failed to find user: {e}"))
}
