// src/authentication/session_store.rs

// dependencies
use actix_session::storage::{
    LoadError, SaveError, SessionKey, SessionStore, UpdateError, generate_session_key,
};
use actix_web::cookie::time::Duration as CookieDuration;
use anyhow::anyhow;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sqlx::PgPool;
use std::collections::HashMap;
use std::net::IpAddr;
use uuid::Uuid;

/// Server-side sessions in Postgres. One row per session: the opaque
/// actix-session state (`state`, JSONB) plus Halation's management metadata
/// (owner, expiry, user agent, ip) that powers the settings-page device list
/// and "log out everywhere".
#[derive(Clone)]
pub struct PostgresSessionStore {
    pool: PgPool,
}

impl PostgresSessionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn expires_at_from(ttl: &CookieDuration) -> Option<DateTime<Utc>> {
        let seconds = ttl.whole_seconds().max(0);
        Some(Utc::now() + ChronoDuration::seconds(seconds))
    }
}

impl SessionStore for PostgresSessionStore {
    async fn load(
        &self,
        session_key: &SessionKey,
    ) -> Result<Option<HashMap<String, String>>, LoadError> {
        let row = sqlx::query_as::<_, (String, Option<DateTime<Utc>>)>(
            "SELECT state::text, expires_at FROM sessions WHERE id = $1",
        )
        .bind(session_key.as_ref())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| LoadError::Other(anyhow!("Failed to load session: {e}")))?;

        match row {
            None => Ok(None),
            Some((state_json, expires_at)) => {
                // An expired row is a miss: sweep it and answer None.
                if expires_at.is_some_and(|at| at < Utc::now()) {
                    sqlx::query("DELETE FROM sessions WHERE id = $1")
                        .bind(session_key.as_ref())
                        .execute(&self.pool)
                        .await
                        .map_err(|e| LoadError::Other(anyhow!("Failed to sweep session: {e}")))?;
                    return Ok(None);
                }

                let state = serde_json::from_str::<HashMap<String, String>>(&state_json)
                    .map_err(|e| LoadError::Deserialization(anyhow!("Invalid session state: {e}")))?;
                Ok(Some(state))
            }
        }
    }

    async fn save(
        &self,
        session_state: HashMap<String, String>,
        ttl: &CookieDuration,
    ) -> Result<SessionKey, SaveError> {
        let session_key = generate_session_key();
        let state_json = serde_json::to_string(&session_state)
            .map_err(|e| SaveError::Serialization(anyhow!("Failed to encode session: {e}")))?;
        let user_id = extracted_uuid(&session_state, "user_id");
        let user_agent = session_state.get("login_user_agent").cloned();
        let ip = extracted_ip(&session_state, "login_ip");

        sqlx::query(
            "INSERT INTO sessions (id, user_id, state, expires_at, last_seen_at, user_agent, ip)
             VALUES ($1, $2, $3::jsonb, $4, now(), $5, $6::inet)",
        )
        .bind(session_key.as_ref())
        .bind(user_id)
        .bind(&state_json)
        .bind(Self::expires_at_from(ttl))
        .bind(user_agent)
        .bind(ip.map(|i| i.to_string()))
        .execute(&self.pool)
        .await
        .map_err(|e| SaveError::Other(anyhow!("Failed to save session: {e}")))?;

        Ok(session_key)
    }

    async fn update(
        &self,
        session_key: SessionKey,
        session_state: HashMap<String, String>,
        ttl: &CookieDuration,
    ) -> Result<SessionKey, UpdateError> {
        let state_json = serde_json::to_string(&session_state)
            .map_err(|e| UpdateError::Serialization(anyhow!("Failed to encode session: {e}")))?;
        let user_id = extracted_uuid(&session_state, "user_id");

        // Upsert: actix may update a session whose row we swept; recreate it.
        sqlx::query(
            "INSERT INTO sessions (id, user_id, state, expires_at, last_seen_at)
             VALUES ($1, $2, $3::jsonb, $4, now())
             ON CONFLICT (id) DO UPDATE
             SET user_id = EXCLUDED.user_id,
                 state = EXCLUDED.state,
                 expires_at = EXCLUDED.expires_at,
                 last_seen_at = now()",
        )
        .bind(session_key.as_ref())
        .bind(user_id)
        .bind(&state_json)
        .bind(Self::expires_at_from(ttl))
        .execute(&self.pool)
        .await
        .map_err(|e| UpdateError::Other(anyhow!("Failed to update session: {e}")))?;

        Ok(session_key)
    }

    async fn update_ttl(
        &self,
        session_key: &SessionKey,
        ttl: &CookieDuration,
    ) -> Result<(), anyhow::Error> {
        sqlx::query("UPDATE sessions SET expires_at = $2 WHERE id = $1")
            .bind(session_key.as_ref())
            .bind(Self::expires_at_from(ttl))
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to refresh session TTL: {e}"))?;
        Ok(())
    }

    async fn delete(&self, session_key: &SessionKey) -> Result<(), anyhow::Error> {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(session_key.as_ref())
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow!("Failed to delete session: {e}"))?;
        Ok(())
    }
}

/// Registration/login handlers plant `user_id` (plus login-time metadata)
/// into the session state; the store lifts it into a queryable column so
/// sessions can be listed and revoked per account.
///
/// Values arrive through two encodings: `Session::insert` JSON-encodes
/// (a Uuid reads back as `\"...\"`), while direct store writes use raw
/// strings. Try JSON first, fall back to raw.
fn extracted_uuid(state: &HashMap<String, String>, key: &str) -> Option<Uuid> {
    let raw = state.get(key)?;
    serde_json::from_str::<Uuid>(raw)
        .ok()
        .or_else(|| raw.parse().ok())
}

fn extracted_ip(state: &HashMap<String, String>, key: &str) -> Option<IpAddr> {
    let raw = state.get(key)?;
    let decoded = serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.clone());
    decoded.parse().ok()
}
