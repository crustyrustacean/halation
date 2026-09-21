// tests/api/helpers.rs

// dependencies
use halation::configuration::{DatabaseSettings, get_configuration};
use halation::startup::{Application, get_connection_pool};
use halation::telemetry::{get_subscriber, init_subscriber};
use secrecy::SecretString;
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
    LazyLock::force(&TRACING);

    let configuration = {
        let mut c = get_configuration().expect("Failed to read configuration.");

        c.database.database_name = Uuid::new_v4().to_string();
        // Keep tests hermetic: no reverse-geocode network calls.
        c.geocode.enabled = false;

        c.application.port = 0;

        c
    };

    configure_database(&configuration.database).await;

    // Launch the application as a background task
    let application = Application::build(configuration.clone())
        .await
        .expect("Failed to build application.");
    let application_port = application.port();
    let _ = tokio::spawn(application.run_until_stopped());

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let test_app = TestApp {
        address: format!("http://localhost:{}", application_port),
        port: application_port,
        db_pool: get_connection_pool(&configuration.database),
        api_client: client,
    };

    test_app
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
    img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
        .unwrap();
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name("photo.jpg")
        .mime_str("image/jpeg")
        .unwrap();

    let response = app
        .api_client
        .post(&format!("{}/upload", &app.address))
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
        .post(&format!("{}/register", &app.address))
        .form(&[("username", username), ("email", email), ("password", password)])
        .send()
        .await
        .expect("registration should succeed");

    let login = app
        .api_client
        .post(&format!("{}/login", &app.address))
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

async fn configure_database(config: &DatabaseSettings) -> PgPool {
    // Create database
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
        .execute(format!(r#"CREATE DATABASE "{}";"#, config.database_name).as_str())
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
