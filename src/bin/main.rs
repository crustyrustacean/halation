// src/bin/main.rs

// dependencies
use halation::authentication::SqliteSessionStore;
use halation::configuration::get_configuration;
use halation::database::{DatabaseBackend, SqliteDatabase};
use halation::services::RateLimiter;
use halation::services::geocode::Geocoder;
use halation::startup::{AUTH_BURST_CAPACITY, AUTH_RATE_PER_SECOND, AppDeps, Application};
use halation::storage::{InMemoryStorageBackend, OpendalStorageBackend, StorageBackend};
use halation::telemetry::{get_subscriber, init_subscriber};
use halation::template::{TemplateRenderer, tera::TeraRenderer};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env if present (dev convenience; silently skipped in production
    // containers where configuration arrives through real env vars).
    let _ = dotenvy::dotenv();

    let subscriber = get_subscriber("halation".into(), "info".into(), std::io::stdout);
    init_subscriber(subscriber);

    let configuration = get_configuration()?;

    let sqlite = SqliteDatabase::connect(&configuration.database).await?;
    let session_store = SqliteSessionStore::new(sqlite.pool());
    let database: Arc<dyn DatabaseBackend> = Arc::new(sqlite);

    let mut site = serde_json::Map::new();
    site.insert(
        "registration_open".into(),
        serde_json::json!(configuration.application.registration_open),
    );
    let template_renderer: Arc<dyn TemplateRenderer> = Arc::new(TeraRenderer::with_site(site)?);

    let storage: Arc<dyn StorageBackend> = match configuration.storage.backend.as_str() {
        "s3" | "r2" => Arc::new(OpendalStorageBackend::new(&configuration.storage)?),
        "memory" => Arc::new(InMemoryStorageBackend::new()),
        other => anyhow::bail!(
            "Unsupported storage backend: '{other}'. Use 'memory', 's3', or 'r2'."
        ),
    };

    let rate_limiter = Arc::new(RateLimiter::new(AUTH_RATE_PER_SECOND, AUTH_BURST_CAPACITY)?);
    let geocoder = Arc::new(Geocoder::new(
        configuration.geocode.enabled,
        configuration.geocode.base_url.clone(),
    ));

    let deps = AppDeps {
        template_renderer,
        database,
        storage,
        session_store,
        rate_limiter,
        geocoder,
    };

    let application = Application::build(configuration, deps).await?;
    application.run_until_stopped().await?;

    Ok(())
}
