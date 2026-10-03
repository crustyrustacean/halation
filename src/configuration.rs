// src/configuration.rs

// dependencies
use secrecy::{ExposeSecret, SecretString};
use serde_aux::field_attributes::deserialize_number_from_string;
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use std::convert::{TryFrom, TryInto};

#[derive(serde::Deserialize, Clone)]
pub struct Settings {
    pub application: ApplicationSettings,
    pub database: DatabaseSettings,
    #[serde(default)]
    pub storage: StorageSettings,
    #[serde(default)]
    pub email: EmailSettings,
    #[serde(default)]
    pub geocode: GeocodeSettings,
    #[serde(default)]
    pub secrets: SecretsSettings,
}

#[derive(serde::Deserialize, Clone)]
pub struct ApplicationSettings {
    #[serde(deserialize_with = "deserialize_number_from_string")]
    pub port: u16,
    pub host: String,
    pub base_url: String,
    /// Open self-service registration? Defaults true (local + tests);
    /// `production.yaml` ships false while the platform is under active
    /// development — existing accounts log in unaffected.
    #[serde(default = "default_registration_open")]
    pub registration_open: bool,
    /// True when this process is serving a test run. Not a config-file
    /// setting: the test harness sets it, and `Application::build` uses it
    /// to refuse a configuration that would reach real infrastructure.
    /// It is deliberately *not* read from the environment — see
    /// `assert_no_remote_storage` for why that matters.
    #[serde(default)]
    pub testing: bool,
    /// Is this process behind a reverse proxy that sets `X-Forwarded-For`?
    ///
    /// Off by default, and deliberately not inferred from the environment.
    /// When off, `client_ip` uses the socket address, which behind a proxy
    /// is the proxy's — shared by every visitor, and therefore useless as a
    /// rate-limit key. When on, the *last* `X-Forwarded-For` entry is used,
    /// since that is the address the trusted hop observed.
    ///
    /// Set it only when a proxy really is in front and really does overwrite
    /// the header. If a client can set the header directly, every visitor
    /// gets their own forged rate-limit bucket.
    #[serde(default)]
    pub trusted_proxy: bool,
}

/// `trusted_proxy` as an Actix extractor target.
///
/// Actix keys `app_data` by the *type* of the value, so two independent
/// `web::Data<bool>` entries are indistinguishable: registering a second one
/// overwrites the first, and whichever handler still asks for `Data<bool>`
/// silently receives the other's value. That is not hypothetical — adding
/// this as a bare `bool` made `/register` start reading `trusted_proxy`
/// (false) and 303 every visitor to `/register/closed`.
///
/// A newtype gives it a distinct `TypeId`. Prefer registering a whole
/// settings struct over adding more bare scalars here.
#[derive(Clone, Copy, Debug)]
pub struct TrustedProxy(pub bool);

fn default_registration_open() -> bool {
    true
}

#[derive(serde::Deserialize, Clone)]
pub struct DatabaseSettings {
    pub username: String,
    pub password: SecretString,
    #[serde(deserialize_with = "deserialize_number_from_string")]
    pub port: u16,
    pub host: String,
    pub database_name: String,
    pub require_ssl: bool,
}

#[derive(serde::Deserialize, Clone, Debug)]
pub struct StorageSettings {
    /// `memory` (default, tests/local) or `s3` (Cloudflare R2 via OpenDAL).
    #[serde(default = "default_storage_backend")]
    pub backend: String,
    #[serde(default)]
    pub r2: R2Settings,
}

/// R2 credentials as a nested group so the env spelling people expect —
/// `APP_STORAGE__R2__BUCKET` — maps to `storage.r2.bucket` directly.
#[derive(serde::Deserialize, Clone, Debug, Default)]
pub struct R2Settings {
    #[serde(default)]
    pub bucket: String,
    /// Optional path prefix inside the bucket.
    #[serde(default)]
    pub fs_root: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub access_key: SecretString,
    #[serde(default)]
    pub secret_key: SecretString,
}

#[derive(serde::Deserialize, Clone, Debug, Default)]
pub struct EmailSettings {
    /// `noop` (default) for v1; the Mailtrap/SMTP backend lands later
    /// behind the same `Mailer` trait.
    #[serde(default = "default_email_backend")]
    pub backend: String,
}

fn default_storage_backend() -> String {
    "memory".to_string()
}

impl Default for StorageSettings {
    /// Matches serde field defaults: the memory backend keeps local runs
    /// and hermetic tests free of any S3 configuration.
    fn default() -> Self {
        Self {
            backend: default_storage_backend(),
            r2: R2Settings::default(),
        }
    }
}

fn default_email_backend() -> String {
    "noop".to_string()
}

#[derive(serde::Deserialize, Clone, Debug, Default)]
pub struct GeocodeSettings {
    /// Reverse geocoding (GPS -> location names) via Nominatim. Enabled by
    /// default: the upload path sends GPS coordinates to the configured
    /// service, best-effort, and degrades to no location when unavailable.
    #[serde(default = "default_geocode_enabled")]
    pub enabled: bool,
    #[serde(default = "default_geocode_base_url")]
    pub base_url: String,
}

fn default_geocode_enabled() -> bool {
    true
}

fn default_geocode_base_url() -> String {
    "https://nominatim.openstreetmap.org".to_string()
}

/// Secret material. The dev default lives in base.yaml; production MUST
/// override via `APP_SECRETS__SESSION_SIGNING_KEY` (>= 64 random bytes).
#[derive(serde::Deserialize, Clone, Default)]
pub struct SecretsSettings {
    pub session_signing_key: SecretString,
}

impl DatabaseSettings {
    pub fn connect_options(&self) -> PgConnectOptions {
        let ssl_mode = if self.require_ssl {
            PgSslMode::Require
        } else {
            PgSslMode::Prefer
        };
        PgConnectOptions::new()
            .host(&self.host)
            .username(&self.username)
            .password(self.password.expose_secret())
            .port(self.port)
            .ssl_mode(ssl_mode)
            .database(&self.database_name)
    }
}

/// The runtime environment name, as resolved by [`get_configuration`].
/// Defaults to `local` when `APP_ENVIRONMENT` is unset.
pub fn current_environment() -> String {
    std::env::var("APP_ENVIRONMENT")
        .unwrap_or_else(|_| "local".into())
        .to_lowercase()
}

/// Storage backends that talk to something outside this process.
const REMOTE_STORAGE_BACKENDS: [&str; 1] = ["s3"];

/// Refuse a configuration that would let a test run reach a real bucket.
///
/// The hazard this guards against is specific and it bit us: a developer's
/// `.env` on the shell sets `APP_STORAGE__BACKEND=s3` with live R2
/// credentials, `cargo test` inherits those variables, and the suite starts
/// writing test JPEGs into the production bucket. Nothing about that is
/// visible from inside the test — the uploads return 500s from a network
/// or permission error, and the failure looks like flakiness.
///
/// The guard lives here rather than in the test harness on purpose: the
/// harness is one caller, but `Application::build` is the single choke
/// point every process passes through, so this also protects a future
/// integration test, a bench, or a manual `cargo run` with `APP_ENVIRONMENT`
/// pointed at a test config.
pub fn assert_no_remote_storage(settings: &Settings) -> Result<(), String> {
    let backend = settings.storage.backend.to_ascii_lowercase();
    if !REMOTE_STORAGE_BACKENDS.contains(&backend.as_str()) {
        return Ok(());
    }

    if !settings.application.testing {
        return Ok(());
    }

    let bucket = if settings.storage.r2.bucket.is_empty() {
        "(unset)".to_string()
    } else {
        settings.storage.r2.bucket.clone()
    };
    let environment = current_environment();
    Err(format!(
        "refusing to start: storage.backend is {backend:?} while application.testing is \
         true (test run, APP_ENVIRONMENT={environment}). That would write real objects \
         into the R2 bucket {bucket:?}. Unset APP_STORAGE__BACKEND (or APP_STORAGE__R2__*) \
         in the environment, or move the credentials out of the shell: the test harness \
         sets the backend to `memory` itself, so a `.env` in the repository root is the \
         usual culprit."
    ))
}

pub fn get_configuration() -> Result<Settings, config::ConfigError> {
    let base_path = std::env::current_dir().expect("Failed to determine the current directory");
    let configuration_directory = base_path.join("configuration");

    // Detect the running environment.
    // Default to `local` if unspecified.
    let environment: Environment = std::env::var("APP_ENVIRONMENT")
        .unwrap_or_else(|_| "local".into())
        .try_into()
        .expect("Failed to parse APP_ENVIRONMENT.");
    let environment_filename = format!("{}.yaml", environment.as_str());
    let settings = config::Config::builder()
        .add_source(config::File::from(
            configuration_directory.join("base.yaml"),
        ))
        .add_source(config::File::from(
            configuration_directory.join(environment_filename),
        ))
        // Add in settings from environment variables (with a prefix of APP and '__' as separator)
        // E.g. `APP_APPLICATION__PORT=5001` would set `Settings.application.port`
        .add_source(
            config::Environment::with_prefix("APP")
                .prefix_separator("_")
                .separator("__"),
        )
        .build()?;

    settings.try_deserialize::<Settings>()
}

/// The possible runtime environment for our application.
pub enum Environment {
    Local,
    Production,
}

impl Environment {
    pub fn as_str(&self) -> &'static str {
        match self {
            Environment::Local => "local",
            Environment::Production => "production",
        }
    }
}

impl TryFrom<String> for Environment {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        match s.to_lowercase().as_str() {
            "local" => Ok(Environment::Local),
            "production" => Ok(Environment::Production),
            other => Err(format!(
                "{} is not a supported environment. Use either `local` or `production`.",
                other
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The env source turns `APP_STORAGE__R2__BUCKET` into the nested path
    /// `storage.r2.bucket` — StorageSettings must deserialize that shape,
    /// or the S3 backend boots with an empty bucket ("The bucket is
    /// misconfigured", OpenDAL ConfigInvalid).
    #[test]
    fn nested_r2_environment_maps_into_storage_settings() {
        // Arrange — the same nested shape the config crate's env source
        // produces for APP_STORAGE__R2__{BUCKET,ENDPOINT,ACCESS_KEY,SECRET_KEY}
        let nested = serde_json::json!({
            "application": {
                "port": 8000,
                "host": "127.0.0.1",
                "base_url": "http://127.0.0.1:8000"
            },
            "database": {
                "host": "127.0.0.1",
                "port": 5433,
                "username": "postgres",
                "password": "password",
                "database_name": "halation",
                "require_ssl": false
            },
            "storage": {
                "backend": "s3",
                "r2": {
                    "bucket": "halation-media",
                    "endpoint": "https://abc123.r2.cloudflarestorage.com",
                    "access_key": "access",
                    "secret_key": "secret"
                }
            },
            "email": { "backend": "noop" },
            "secrets": { "session_signing_key": "test-key" }
        });

        // Act
        let settings: Settings = serde_json::from_value(nested).expect("should deserialize");

        // Assert
        assert_eq!("s3", settings.storage.backend);
        assert_eq!("halation-media", settings.storage.r2.bucket);
        assert_eq!(
            "https://abc123.r2.cloudflarestorage.com",
            settings.storage.r2.endpoint
        );
        assert_eq!("access", settings.storage.r2.access_key.expose_secret());
        assert_eq!("secret", settings.storage.r2.secret_key.expose_secret());
    }

    /// Defaults hold when the nested r2 section is absent entirely.
    #[test]
    fn storage_defaults_without_r2_section() {
        // Arrange
        let minimal = serde_json::json!({
            "application": {
                "port": 8000, "host": "127.0.0.1", "base_url": "http://127.0.0.1:8000"
            },
            "database": {
                "host": "127.0.0.1", "port": 5433, "username": "postgres",
                "password": "password", "database_name": "halation", "require_ssl": false
            }
        });

        // Act
        let settings: Settings = serde_json::from_value(minimal).expect("should deserialize");

        // Assert — memory backend, empty r2 credentials
        assert_eq!("memory", settings.storage.backend);
        assert_eq!("", settings.storage.r2.bucket);
        assert_eq!("", settings.storage.r2.access_key.expose_secret());
        // registration_open defaults true when the key is absent entirely
        assert!(settings.application.registration_open);
        // and `testing` defaults false, so a config file cannot opt itself
        // into test mode
        assert!(!settings.application.testing);
    }

    // --- the hermetic-test guard ------------------------------------------

    fn settings_with(storage_backend: &str, testing: bool) -> Settings {
        serde_json::from_value(serde_json::json!({
            "application": {
                "port": 8000, "host": "127.0.0.1", "base_url": "http://127.0.0.1:8000",
                "testing": testing
            },
            "database": {
                "host": "127.0.0.1", "port": 5433, "username": "postgres",
                "password": "password", "database_name": "halation", "require_ssl": false
            },
            "storage": {
                "backend": storage_backend,
                "r2": {
                    "bucket": "halation-photos",
                    "endpoint": "https://example.r2.cloudflarestorage.com",
                    "access_key": "live-access-key",
                    "secret_key": "live-secret-key"
                }
            }
        }))
        .expect("should deserialize")
    }

    /// The exact failure that motivated the guard: a test run configured
    /// with live R2 credentials must be refused, loudly, before boot.
    #[test]
    fn test_run_with_s3_storage_is_refused() {
        // Arrange — what a leaked `.env` produces
        let settings = settings_with("s3", true);

        // Act
        let result = assert_no_remote_storage(&settings);

        // Assert
        let message = result.expect_err("a test run must not reach R2");
        assert!(message.contains("s3"), "names the backend: {message}");
        assert!(
            message.contains("halation-photos"),
            "names the bucket at risk: {message}"
        );
        // The message must not echo the credentials back.
        assert!(!message.contains("live-secret-key"), "{message}");
    }

    /// The other half: production is *supposed* to use s3, so the guard
    /// must not fire there. A false positive here would make the deployed
    /// app unbootable.
    #[test]
    fn non_test_run_with_s3_storage_is_allowed() {
        // Arrange
        let settings = settings_with("s3", false);

        // Act / Assert
        assert_no_remote_storage(&settings).expect("production must be allowed to use s3");
    }

    #[test]
    fn test_run_with_memory_storage_is_allowed() {
        // Arrange — the shape the harness actually builds
        let mut settings = settings_with("memory", true);

        // Act / Assert
        assert_no_remote_storage(&settings).expect("memory storage is hermetic");
        // and the credentials are irrelevant once the backend is memory
        settings.storage.r2.bucket = "halation-photos".to_string();
        assert_no_remote_storage(&settings).expect("only the backend matters");
    }

    /// The backend name is compared case-insensitively: `S3` from an env
    /// var must not slip past the guard.
    #[test]
    fn backend_matching_is_case_insensitive() {
        // Arrange
        let settings = settings_with("S3", true);

        // Act / Assert
        assert!(
            assert_no_remote_storage(&settings).is_err(),
            "`S3` must be treated the same as `s3`"
        );
    }
}
