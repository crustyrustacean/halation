// tests/api/bootstrap.rs

use halation::authentication::SqliteSessionStore;
use halation::configuration::get_configuration;
use halation::database::{DatabaseBackend, SqliteDatabase};
use halation::services::RateLimiter;
use halation::services::geocode::Geocoder;
use halation::startup::{AUTH_BURST_CAPACITY, AUTH_RATE_PER_SECOND, AppDeps, Application};
use halation::storage::{InMemoryStorageBackend, StorageBackend};
use halation::template::{TemplateRenderer, tera::TeraRenderer};
use std::sync::Arc;
use uuid::Uuid;

/// `cargo run` on a fresh machine must bootstrap its own database:
/// create parent directories + SQLite file if missing, apply all migrations,
/// and be ready to serve.
#[tokio::test]
async fn build_bootstraps_a_missing_database() {
    // Arrange — a SQLite path in a directory that does not exist yet
    let dir = std::env::temp_dir().join(format!("halation-bootstrap-{}", Uuid::new_v4()));
    let db_path = dir.join("halation.db");
    let mut configuration = get_configuration().expect("Failed to read configuration.");
    configuration.database.path = db_path.to_string_lossy().into_owned();
    configuration.application.port = 0;
    configuration.application.testing = true;
    configuration.storage.backend = "memory".to_string();

    // Act — connect bootstraps the missing directory/file and runs migrations
    let sqlite = SqliteDatabase::connect(&configuration.database)
        .await
        .expect("connect should create and migrate a missing database");
    let pool = sqlite.pool();
    let deps = AppDeps {
        template_renderer: Arc::new(
            TeraRenderer::with_site(serde_json::Map::new()).expect("renderer"),
        ) as Arc<dyn TemplateRenderer>,
        database: Arc::new(sqlite) as Arc<dyn DatabaseBackend>,
        storage: Arc::new(InMemoryStorageBackend::new()) as Arc<dyn StorageBackend>,
        session_store: SqliteSessionStore::new(pool.clone()),
        rate_limiter: Arc::new(
            RateLimiter::new(AUTH_RATE_PER_SECOND, AUTH_BURST_CAPACITY).expect("rate limiter"),
        ),
        geocoder: Arc::new(Geocoder::new(false, configuration.geocode.base_url.clone())),
    };
    let application = Application::build(configuration, deps)
        .await
        .expect("build should succeed with bootstrapped deps");
    tokio::spawn(application.run_until_stopped());

    // Assert — every shipped table exists in the freshly bootstrapped DB
    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master \
         WHERE type = 'table' \
           AND name IN ('users', 'sessions', 'media', 'posts')",
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to query the bootstrapped database");
    assert_eq!(4, tables, "all shipped tables exist after bootstrap");

    let _ = std::fs::remove_dir_all(&dir);
}
