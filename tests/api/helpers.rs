// tests/api/helpers.rs

// dependencies
use halation::authentication::SqliteSessionStore;
use halation::configuration::get_configuration;
use halation::database::{DatabaseBackend, SqliteDatabase};
use halation::services::RateLimiter;
use halation::services::geocode::Geocoder;
use halation::startup::{AUTH_BURST_CAPACITY, AUTH_RATE_PER_SECOND, AppDeps, Application};
use halation::storage::{InMemoryStorageBackend, StorageBackend};
use halation::telemetry::{get_subscriber, init_subscriber};
use halation::template::{TemplateRenderer, tera::TeraRenderer};
use secrecy::ExposeSecret;
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use uuid::Uuid;

// Ensure that the `tracing` stack is only initialised once using `LazyLock`
static TRACING: LazyLock<()> = LazyLock::new(|| {
    let default_filter_level = "info".to_string();
    let subscriber_name = "test".to_string();
    if std::env::var("TEST_LOG").is_ok() {
        let subscriber = get_subscriber(subscriber_name, default_filter_level, std::io::stdout);
        init_subscriber(subscriber);
    } else {
        let subscriber = get_subscriber(subscriber_name, default_filter_level, std::io::sink);
        init_subscriber(subscriber);
    };
});

#[allow(dead_code)]
pub struct TestApp {
    pub address: String,
    pub port: u16,
    pub db_pool: SqlitePool,
    pub api_client: reqwest::Client,
    db_path: PathBuf,
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.db_path);
        let _ = std::fs::remove_file(format!("{}-wal", self.db_path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", self.db_path.display()));
    }
}

pub async fn spawn_app() -> TestApp {
    spawn_app_with(Box::new(|_| {})).await
}

/// Spawn a test app after tweaking the configuration (e.g. closing
/// registration) — the same hermetic setup as `spawn_app`.
pub async fn spawn_app_with(
    configure: Box<dyn FnOnce(&mut halation::configuration::Settings)>,
) -> TestApp {
    LazyLock::force(&TRACING);

    let db_path = std::env::temp_dir().join(format!("halation-test-{}.db", Uuid::new_v4()));

    let mut configuration = {
        let mut c = get_configuration().expect("Failed to read configuration.");

        c.database.path = db_path.to_string_lossy().into_owned();
        // Keep tests hermetic: no reverse-geocode network calls.
        c.geocode.enabled = false;

        c.application.port = 0;

        // --- Hermetic test configuration -------------------------------
        c.application.testing = true;
        c.storage.backend = "memory".to_string();
        c.storage.r2 = Default::default();
        c.email.backend = "noop".to_string();

        c
    };
    configure(&mut configuration);

    // The per-test closure is a test's own business, but it must not be
    // able to undo the guarantees above. Re-assert after it runs.
    assert_test_configuration_is_local(&configuration);

    // Construct injected dependencies (matching main.rs) and launch the server.
    let sqlite = SqliteDatabase::connect(&configuration.database)
        .await
        .expect("Failed to initialise test SQLite database.");
    let db_pool = sqlite.pool();
    let session_store = SqliteSessionStore::new(db_pool.clone());
    let database: Arc<dyn DatabaseBackend> = Arc::new(sqlite);

    let mut site = serde_json::Map::new();
    site.insert(
        "registration_open".into(),
        serde_json::json!(configuration.application.registration_open),
    );
    let template_renderer: Arc<dyn TemplateRenderer> =
        Arc::new(TeraRenderer::with_site(site).expect("Failed to build template renderer."));
    let storage: Arc<dyn StorageBackend> = Arc::new(InMemoryStorageBackend::new());
    let rate_limiter = Arc::new(
        RateLimiter::new(AUTH_RATE_PER_SECOND, AUTH_BURST_CAPACITY)
            .expect("Failed to build rate limiter."),
    );
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

    let application = Application::build(configuration, deps)
        .await
        .expect("Failed to build application.");
    let application_port = application.port();
    tokio::spawn(application.run_until_stopped());

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    TestApp {
        address: format!("http://localhost:{}", application_port),
        port: application_port,
        db_pool,
        api_client: client,
        db_path,
    }
}

/// Upload one photo as the cookie's user; returns the permalink path.
pub async fn upload_and_get_location(
    app: &TestApp,
    cookie: &str,
    caption: &str,
    width: u32,
    height: u32,
) -> String {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    }));
    let mut bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Jpeg,
    )
    .unwrap();
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name("photo.jpg")
        .mime_str("image/jpeg")
        .unwrap();

    let response = app
        .api_client
        .post(format!("{}/upload", app.address))
        .header("Cookie", cookie)
        .multipart(
            reqwest::multipart::Form::new()
                .text("caption", caption.to_string())
                .part("files", part),
        )
        .send()
        .await
        .expect("upload should succeed");
    assert_eq!(303, response.status().as_u16());
    response
        .headers()
        .get("Location")
        .and_then(|v| v.to_str().ok())
        .expect("upload should redirect")
        .to_string()
}

/// Register a fresh user and return their session cookie string.
pub async fn register_and_login(
    app: &TestApp,
    username: &str,
    email: &str,
    password: &str,
) -> String {
    app.api_client
        .post(format!("{}/register", app.address))
        .form(&[
            ("username", username),
            ("email", email),
            ("password", password),
        ])
        .send()
        .await
        .expect("registration should succeed");

    let login = app
        .api_client
        .post(format!("{}/login", app.address))
        .form(&[("identifier", username), ("password", password)])
        .send()
        .await
        .expect("login should succeed");

    login
        .headers()
        .get_all("Set-Cookie")
        .iter()
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Fail loudly if anything about this test run could touch real
/// infrastructure, or if the surrounding environment is a production one.
fn assert_test_configuration_is_local(settings: &halation::configuration::Settings) {
    // 1. Storage must be in-process.
    assert_eq!(
        "memory", settings.storage.backend,
        "tests must not touch real object storage; storage.backend was forced to `memory` \
         by the harness. If you are seeing this from a per-test `configure` closure, it \
         must not set storage.backend."
    );
    assert!(
        settings.storage.r2.bucket.is_empty()
            && settings.storage.r2.endpoint.is_empty()
            && settings.storage.r2.access_key.expose_secret().is_empty()
            && settings.storage.r2.secret_key.expose_secret().is_empty(),
        "tests must not carry R2 credentials; the harness clears them. A populated \
         r2 section means the environment leaked in."
    );

    // 2. The environment itself must not be production.
    let environment = halation::configuration::current_environment();
    assert_ne!(
        "production", environment,
        "APP_ENVIRONMENT=production in the environment: tests would load \
         production.toml. Unset it, or export APP_ENVIRONMENT=local."
    );

    // 3. The database must be a throwaway test file, not the dev `data/halation.db`.
    assert!(
        settings.database.path.contains("halation-test-"),
        "tests must run against a throwaway SQLite file, but database.path is {:?}.",
        settings.database.path
    );
}
