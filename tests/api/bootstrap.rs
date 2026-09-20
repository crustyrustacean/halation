// tests/api/bootstrap.rs

use halation::configuration::get_configuration;
use halation::startup::{Application, get_connection_pool};
use uuid::Uuid;

/// `cargo run` on a fresh machine must bootstrap its own database:
/// create it if missing, apply all migrations, and be ready to serve.
#[tokio::test]
async fn build_bootstraps_a_missing_database() {
    // Arrange — a database name that does not exist yet (no pre-creation,
    // unlike the other tests which use spawn_app)
    let mut configuration = get_configuration().expect("Failed to read configuration.");
    configuration.database.database_name = Uuid::new_v4().to_string();
    configuration.application.port = 0;

    // Act
    let application = Application::build(configuration.clone())
        .await
        .expect("build should create and migrate a missing database");
    let _ = tokio::spawn(application.run_until_stopped());

    // Assert — every shipped table exists in the freshly bootstrapped DB
    let pool = get_connection_pool(&configuration.database);
    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = 'public' \
           AND table_name IN ('users', 'sessions', 'media', 'posts')",
    )
    .fetch_one(&pool)
    .await
    .expect("Failed to query the bootstrapped database");
    assert_eq!(4, tables, "all shipped tables exist after bootstrap");
}
