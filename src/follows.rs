// src/follows.rs
//!
//! The social graph's shared types. Every SQL statement that reads or
//! writes `follows` lives in `database::postgres`, behind the
//! `DatabaseBackend` trait.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AccountSummary {
    pub username: String,
    pub display_name: String,
    /// First letter of the username, uppercased — the avatar initial.
    pub initial: String,
}
