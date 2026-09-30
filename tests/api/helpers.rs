// tests/api/helpers.rs

// dependencies
use halation::configuration::{DatabaseSettings, get_configuration};
use halation::startup::{Application, get_connection_pool};
use halation::telemetry::{get_subscriber, init_subscriber};
use secrecy::{ExposeSecret, SecretString};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use std::sync::LazyLock;
use uuid::Uuid;

// Ensure that the `tracing` stack is only initialised once using `once_cell`
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
    pub db_pool: PgPool,
    pub api_client: reqwest::Client,
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

    let mut configuration = {
        let mut c = get_configuration().expect("Failed to read configuration.");

        c.database.database_name = Uuid::new_v4().to_string();
        // Keep tests hermetic: no reverse-geocode network calls.
        c.geocode.enabled = false;

        c.application.port = 0;

        // --- Hermetic test configuration -------------------------------
        //
        // Everything below overrides whatever the surrounding environment
        // says, so a developer's `.env` or exported `APP_*` variables can
        // never redirect a test at real infrastructure. This is a second
        // line of defence: `Application::build` also refuses a remote
        // storage backend outright when `testing` is set, and fails loudly
        // rather than silently succeeding against production.
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

    configure_database(&configuration.database).await;

    // Launch the application as a background task
    let application = Application::build(configuration.clone())
        .await
        .expect("Failed to build application.");
    let application_port = application.port();
    // Spawn and detach: the JoinHandle is dropped, the server keeps running.
    tokio::spawn(application.run_until_stopped());

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    TestApp {
        address: format!("http://localhost:{}", application_port),
        port: application_port,
        db_pool: get_connection_pool(&configuration.database),
        api_client: client,
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
///
/// Two distinct hazards, both observed in practice:
///
/// 1. `APP_STORAGE__BACKEND=s3` (plus credentials) leaking in from a `.env`
///    or an exported variable, which sends test uploads to the live bucket.
/// 2. `APP_ENVIRONMENT=production` in the shell, which loads
///    `production.yaml` and can disable registration or point at other
///    production-only settings — the suite would then be testing the
///    production configuration while believing it tests the defaults.
///
/// This runs before the app boots, so a mistake is a clear error message
/// rather than a confusing 500 three layers deeper.
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
         production.yaml. Unset it, or export APP_ENVIRONMENT=local."
    );

    // 3. The database must be a throwaway one, not the real `halation`.
    let name = &settings.database.database_name;
    let looks_like_throwaway = Uuid::parse_str(name).is_ok();
    assert!(
        looks_like_throwaway,
        "tests must run against a throwaway database, but database_name is {name:?}. \
         The harness assigns a UUID; something overwrote it."
    );
}

async fn configure_database(config: &DatabaseSettings) -> PgPool {
    let maintenance_settings = DatabaseSettings {
        database_name: "postgres".to_string(),
        username: "postgres".to_string(),
        password: SecretString::new("password".into()),
        ..config.clone()
    };
    let mut connection = PgConnection::connect_with(&maintenance_settings.connect_options())
        .await
        .expect("Failed to connect to Postgres");
    connection
        // Test-database name, generated as a UUID by `spawn_app`.
        .execute(sqlx::AssertSqlSafe(format!(
            r#"CREATE DATABASE "{}";"#,
            config.database_name
        )))
        .await
        .expect("Failed to create database.");

    // Migrate database
    let connection_pool = PgPool::connect_with(config.connect_options())
        .await
        .expect("Failed to connect to Postgres.");
    sqlx::migrate!("./migrations")
        .run(&connection_pool)
        .await
        .expect("Failed to migrate the database");
    connection_pool
}
