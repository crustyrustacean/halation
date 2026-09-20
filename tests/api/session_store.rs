// tests/api/session_store.rs

use crate::helpers::spawn_app;
use actix_session::storage::{SessionKey, SessionStore};
use halation::authentication::PostgresSessionStore;
use uuid::Uuid;
use std::collections::HashMap;

#[tokio::test]
async fn session_store_round_trips_through_postgres() {
    // Arrange — the spawned app's own pool, so the store writes to the
    // same throwaway database the harness created.
    let app = spawn_app().await;
    let store = PostgresSessionStore::new(app.db_pool.clone());

    // A session belongs to a real account: create one to satisfy the FK.
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, password_hash)
         VALUES ('storetest', 'storetest@example.com', 'not-a-real-hash')
         RETURNING id",
    )
    .fetch_one(&app.db_pool)
    .await
    .expect("user seed should succeed");

    let mut state: HashMap<String, String> = HashMap::new();
    state.insert("user_id".to_string(), user_id.to_string());
    state.insert("theme".to_string(), "dark".to_string());
    let ttl = actix_web::cookie::time::Duration::minutes(15);

    // Act — save → load
    let key = store
        .save(state.clone(), &ttl)
        .await
        .expect("save should succeed");

    let loaded = store
        .load(&key)
        .await
        .expect("load should succeed")
        .expect("a freshly saved session must load");

    // Assert — the state round-trips intact
    assert_eq!(state, loaded);
    let persisted: (Option<Uuid>, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT user_id, expires_at FROM sessions WHERE id = $1")
            .bind(key.as_ref())
            .fetch_one(&app.db_pool)
            .await
            .expect("row should exist");
    assert_eq!(Some(user_id), persisted.0, "user_id is lifted into its column");
    assert!(persisted.1.is_some(), "the ttl became an expiry timestamp");

    // SessionKey is neither Clone nor Copy — hold the string, rebuild keys.
    let key_string = key.as_ref().to_string();
    let rebuild = |s: &str| {
        SessionKey::try_from(s.to_string()).expect("round-tripped key is still valid")
    };

    // Act — update changes the state in place
    let mut updated = state.clone();
    updated.insert("theme".to_string(), "light".to_string());
    store
        .update(rebuild(&key_string), updated.clone(), &ttl)
        .await
        .expect("update should succeed");
    let reloaded = store
        .load(&rebuild(&key_string))
        .await
        .expect("load should succeed")
        .expect("session still exists");
    assert_eq!(updated, reloaded);

    // Act — update_ttl pushes expiry out; delete removes the row
    store
        .update_ttl(&rebuild(&key_string), &ttl)
        .await
        .expect("ttl update should succeed");
    store
        .delete(&rebuild(&key_string))
        .await
        .expect("delete should succeed");
    let gone = store
        .load(&rebuild(&key_string))
        .await
        .expect("load should succeed");
    assert!(gone.is_none(), "a deleted session no longer loads");
}

#[tokio::test]
async fn expired_sessions_are_swept_on_load() {
    // Arrange
    let app = spawn_app().await;
    let store = PostgresSessionStore::new(app.db_pool.clone());

    let state: HashMap<String, String> =
        HashMap::from([("k".to_string(), "v".to_string())]);
    let negative_ttl = actix_web::cookie::time::Duration::seconds(-1);

    // Act — save with an already-elapsed ttl, then load
    let key = store
        .save(state, &negative_ttl)
        .await
        .expect("save should succeed");
    let loaded = store.load(&key).await.expect("load should succeed");

    // Assert — expired records are misses, and the row is swept
    assert!(loaded.is_none());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&app.db_pool)
        .await
        .unwrap();
    assert_eq!(0, count, "the sweep removed the expired row");
}
