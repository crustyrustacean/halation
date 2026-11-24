// src/configuration.rs

// dependencies
use confik::{Configuration, EnvSource, FileSource};
use secrecy::{ExposeSecret, SecretString};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use std::convert::{TryFrom, TryInto};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

#[derive(Configuration, serde::Deserialize, Clone)]
pub struct Settings {
    pub application: ApplicationSettings,
    #[serde(default)]
    #[confik(default)]
    pub database: DatabaseSettings,
    #[serde(default)]
    #[confik(default)]
    pub storage: StorageSettings,
    #[serde(default)]
    #[confik(default)]
    pub email: EmailSettings,
    #[serde(default)]
    #[confik(default)]
    pub geocode: GeocodeSettings,
    #[serde(default)]
    #[confik(default)]
    pub secrets: SecretsSettings,
}

#[derive(Configuration, serde::Deserialize, Clone)]
pub struct ApplicationSettings {
    pub port: u16,
    pub host: String,
    pub base_url: String,
    /// Directory served under `/static`.
    #[serde(default = "default_static_dir")]
    #[confik(default = "static")]
    pub static_dir: String,
    /// Open self-service registration? Defaults true (local + tests);
    /// `production.toml` ships false while the platform is under active
    /// development — existing accounts log in unaffected.
    #[serde(default = "default_registration_open")]
    #[confik(default = true)]
    pub registration_open: bool,
    /// True when this process is serving a test run. Not a config-file
    /// setting: the test harness sets it, and `Application::build` uses it
    /// to refuse a configuration that would reach real infrastructure.
    /// It is deliberately *not* read from the environment — see
    /// `assert_no_remote_storage` for why that matters.
    #[serde(default)]
    #[confik(default)]
    pub testing: bool,
    /// Is this process behind a reverse proxy that sets `Fly-Client-IP` or
    /// `X-Forwarded-For`?
    ///
    /// Off by default, and deliberately not inferred from the environment.
    /// When off, `client_ip` uses the socket address, which behind a proxy
    /// is the proxy's — shared by every visitor, and therefore useless as a
    /// rate-limit key. When on, `Fly-Client-IP` is preferred, falling back
    /// to the *last* `X-Forwarded-For` entry.
    ///
    /// Set it only when a proxy really is in front and really does overwrite
    /// the header. If a client can set the header directly, every visitor
    /// gets their own forged rate-limit bucket.
    #[serde(default)]
    #[confik(default)]
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

fn default_static_dir() -> String {
    "static".to_string()
}

#[derive(Configuration, serde::Deserialize, Clone, Debug)]
pub struct DatabaseSettings {
    /// SQLite database path (e.g. `data/halation.db`, `/data/halation.db`, or
    /// `:memory:`). Also accepts a `sqlite://...` URI.
    #[serde(default = "default_database_path")]
    #[confik(default = "data/halation.db")]
    pub path: String,
    /// Optional SQLite connection pool cap. Defaults to 5 for file databases
    /// and 1 for `:memory:`.
    #[serde(default)]
    #[confik(default)]
    pub max_connections: Option<u32>,
}

fn default_database_path() -> String {
    "data/halation.db".to_string()
}

impl Default for DatabaseSettings {
    fn default() -> Self {
        Self {
            path: default_database_path(),
            max_connections: None,
        }
    }
}

impl DatabaseSettings {
    pub fn connect_options(&self) -> SqliteConnectOptions {
        let raw = self.path.trim();
        let base = if raw.starts_with("sqlite:") {
            SqliteConnectOptions::from_str(raw)
                .expect("database.path must be a valid SQLite URI or path")
        } else if raw == ":memory:" {
            SqliteConnectOptions::new().in_memory(true)
        } else {
            SqliteConnectOptions::new().filename(raw)
        };

        base.create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5))
    }

    /// Filesystem path to the SQLite file, or `None` for `:memory:`.
    pub fn file_path(&self) -> Option<&Path> {
        let raw = self.path.trim();
        if raw.is_empty() || raw == ":memory:" || raw == "sqlite::memory:" {
            return None;
        }
        let stripped = raw
            .strip_prefix("sqlite://")
            .or_else(|| raw.strip_prefix("sqlite:"))
            .unwrap_or(raw);
        let path_only = stripped.split('?').next().unwrap_or(stripped);
        if path_only.is_empty() || path_only == ":memory:" {
            None
        } else {
            Some(Path::new(path_only))
        }
    }

    pub fn is_in_memory(&self) -> bool {
        self.file_path().is_none()
    }
}

#[derive(Configuration, serde::Deserialize, Clone, Debug)]
pub struct StorageSettings {
    /// `memory` (default, tests/local) or `s3` (Cloudflare R2 / Tigris via OpenDAL).
    #[serde(default = "default_storage_backend")]
    #[confik(default = "memory")]
    pub backend: String,
    #[serde(default)]
    #[confik(default)]
    pub r2: R2Settings,
}

/// R2 / S3 credentials as a nested group so the env spelling people expect —
/// `APP_STORAGE__R2__BUCKET` — maps to `storage.r2.bucket` directly.
#[derive(Configuration, serde::Deserialize, Clone, Debug, Default)]
pub struct R2Settings {
    #[serde(default)]
    #[confik(default)]
    pub bucket: String,
    /// Optional path prefix inside the bucket.
    #[serde(default)]
    #[confik(default)]
    pub fs_root: String,
    #[serde(default)]
    #[confik(default)]
    pub endpoint: String,
    #[serde(default)]
    #[confik(default)]
    pub access_key: SecretString,
    #[serde(default)]
    #[confik(default)]
    pub secret_key: SecretString,
}

#[derive(Configuration, serde::Deserialize, Clone, Debug, Default)]
pub struct EmailSettings {
    /// `noop` (default) for v1; the Mailtrap/SMTP backend lands later
    /// behind the same `Mailer` trait.
    #[serde(default = "default_email_backend")]
    #[confik(default = "noop")]
    pub backend: String,
}

fn default_storage_backend() -> String {
    "memory".to_string()
}

impl Default for StorageSettings {
    /// Matches field defaults: the memory backend keeps local runs
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

#[derive(Configuration, serde::Deserialize, Clone, Debug)]
pub struct GeocodeSettings {
    /// Reverse geocoding (GPS -> location names) via Nominatim. Enabled by
    /// default: the upload path sends GPS coordinates to the configured
    /// service, best-effort, and degrades to no location when unavailable.
    #[serde(default = "default_geocode_enabled")]
    #[confik(default = true)]
    pub enabled: bool,
    #[serde(default = "default_geocode_base_url")]
    #[confik(default = "https://nominatim.openstreetmap.org")]
    pub base_url: String,
}

impl Default for GeocodeSettings {
    fn default() -> Self {
        Self {
            enabled: default_geocode_enabled(),
            base_url: default_geocode_base_url(),
        }
    }
}

fn default_geocode_enabled() -> bool {
    true
}

fn default_geocode_base_url() -> String {
    "https://nominatim.openstreetmap.org".to_string()
}

/// Secret material. The dev default lives in `configuration/local.toml`;
/// production MUST provide `APP_SECRETS__SESSION_SIGNING_KEY` (>= 64 random
/// bytes) via environment variables.
#[derive(Configuration, serde::Deserialize, Clone, Default)]
pub struct SecretsSettings {
    #[serde(default)]
    #[confik(default)]
    pub session_signing_key: SecretString,
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

/// Minimum signing key length required by `actix_session::SessionMiddleware`
/// (`cookie::Key::from` panics below 64 bytes).
const MIN_SESSION_KEY_LEN: usize = 64;

/// Prefix of the checked-in dev signing key in `configuration/local.toml`.
const DEV_SESSION_KEY_PREFIX: &str = "dev-only-key-";

/// Fail fast on misconfigured production secrets or database paths before
/// binding a socket or touching external services.
pub fn assert_production_ready(settings: &Settings, environment: &str) -> Result<(), String> {
    let db_path = settings.database.path.trim();
    if db_path.is_empty() {
        return Err("database.path must not be empty".to_string());
    }
    if db_path.starts_with("sqlite:") {
        SqliteConnectOptions::from_str(db_path)
            .map_err(|e| format!("invalid SQLite URI in database.path: {e}"))?;
    }

    if environment.eq_ignore_ascii_case("production") && !settings.application.testing {
        let key = settings.secrets.session_signing_key.expose_secret();
        if key.is_empty() || key.starts_with(DEV_SESSION_KEY_PREFIX) {
            return Err(
                "refusing to start in production with the default dev session signing key. \
                 Set APP_SECRETS__SESSION_SIGNING_KEY to at least 64 random bytes \
                 (e.g. `openssl rand -hex 48`)."
                    .to_string(),
            );
        }
        if key.len() < MIN_SESSION_KEY_LEN {
            return Err(format!(
                "refusing to start in production: APP_SECRETS__SESSION_SIGNING_KEY must be \
                 at least {MIN_SESSION_KEY_LEN} bytes (got {}). Generate one with \
                 `openssl rand -hex 48`.",
                key.len()
            ));
        }
    }

    Ok(())
}

pub fn get_configuration() -> Result<Settings, confik::Error> {
    let base_path = std::env::current_dir().expect("Failed to determine the current directory");
    let configuration_directory = base_path.join("configuration");

    // Detect the running environment.
    // Default to `local` if unspecified.
    let environment: Environment = std::env::var("APP_ENVIRONMENT")
        .unwrap_or_else(|_| "local".into())
        .try_into()
        .expect("Failed to parse APP_ENVIRONMENT.");
    let environment_filename = format!("{}.toml", environment.as_str());

    // `base.toml` and `production.toml` are loaded without `.allow_secrets()`,
    // so confik rejects any secret accidentally placed in them. `local.toml`
    // opts in so local dev and tests work out of the box.
    let base_source = FileSource::new(configuration_directory.join("base.toml"));
    let env_file_path = configuration_directory.join(environment_filename);
    let env_file_source = match environment {
        Environment::Local => FileSource::new(env_file_path).allow_secrets(),
        Environment::Production => FileSource::new(env_file_path),
    };

    // Environment variables use `APP_` prefix and `__` nesting separator,
    // e.g. `APP_APPLICATION__PORT=5001` -> `Settings.application.port`.
    let parsed = Settings::builder()
        .override_with(base_source)
        .override_with(env_file_source)
        .override_with(
            EnvSource::new()
                .with_prefix("APP_")
                .with_separator("__")
                .allow_secrets(),
        )
        .try_build()?;

    Ok(parsed)
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
    use confik::TomlSource;

    #[test]
    fn nested_r2_environment_maps_into_storage_settings() {
        let nested = serde_json::json!({
            "application": {
                "port": 8000,
                "host": "127.0.0.1",
                "base_url": "http://127.0.0.1:8000"
            },
            "database": {
                "path": "data/halation.db"
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

        let settings: Settings = serde_json::from_value(nested).expect("should deserialize");

        assert_eq!("s3", settings.storage.backend);
        assert_eq!("halation-media", settings.storage.r2.bucket);
        assert_eq!(
            "https://abc123.r2.cloudflarestorage.com",
            settings.storage.r2.endpoint
        );
        assert_eq!("access", settings.storage.r2.access_key.expose_secret());
        assert_eq!("secret", settings.storage.r2.secret_key.expose_secret());
    }

    #[test]
    fn storage_defaults_without_r2_section() {
        let minimal = serde_json::json!({
            "application": {
                "port": 8000, "host": "127.0.0.1", "base_url": "http://127.0.0.1:8000"
            }
        });

        let settings: Settings = serde_json::from_value(minimal).expect("should deserialize");

        assert_eq!("memory", settings.storage.backend);
        assert_eq!("", settings.storage.r2.bucket);
        assert_eq!("", settings.storage.r2.access_key.expose_secret());
        assert_eq!("data/halation.db", settings.database.path);
        assert!(settings.application.registration_open);
        assert!(!settings.application.testing);
    }

    #[test]
    fn checked_in_toml_configs_load_cleanly() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("configuration");

        let local = Settings::builder()
            .override_with(FileSource::new(dir.join("base.toml")))
            .override_with(FileSource::new(dir.join("local.toml")).allow_secrets())
            .try_build()
            .expect("base.toml + local.toml must build");
        assert_eq!("127.0.0.1", local.application.host);
        assert_eq!(8000, local.application.port);
        assert!(local.application.registration_open);
        assert!(!local.application.trusted_proxy);
        assert_eq!("data/halation.db", local.database.path);
        assert!(!local.secrets.session_signing_key.expose_secret().is_empty());

        let prod = Settings::builder()
            .override_with(FileSource::new(dir.join("base.toml")))
            .override_with(FileSource::new(dir.join("production.toml")))
            .try_build()
            .expect("base.toml + production.toml must build without secrets");
        assert_eq!("0.0.0.0", prod.application.host);
        assert!(!prod.application.registration_open);
        assert!(prod.application.trusted_proxy);
        assert_eq!("/data/halation.db", prod.database.path);
    }

    #[test]
    fn confik_rejects_secrets_in_untrusted_toml_source() {
        let toml_with_secret = r#"
            [application]
            port = 8000
            host = "0.0.0.0"
            base_url = "https://halation.photos"

            [secrets]
            session_signing_key = "leaked-signing-key"
        "#;

        let err = Settings::builder()
            .override_with(TomlSource::new(toml_with_secret))
            .try_build();
        assert!(
            err.is_err(),
            "a TomlSource without .allow_secrets() must reject SecretString fields"
        );
    }

    fn settings_with(storage_backend: &str, testing: bool) -> Settings {
        serde_json::from_value(serde_json::json!({
            "application": {
                "port": 8000, "host": "127.0.0.1", "base_url": "http://127.0.0.1:8000",
                "testing": testing
            },
            "database": {
                "path": ":memory:"
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

    #[test]
    fn test_run_with_s3_storage_is_refused() {
        let settings = settings_with("s3", true);
        let message =
            assert_no_remote_storage(&settings).expect_err("a test run must not reach R2");
        assert!(message.contains("s3"), "names the backend: {message}");
        assert!(
            message.contains("halation-photos"),
            "names the bucket at risk: {message}"
        );
        assert!(!message.contains("live-secret-key"), "{message}");
    }

    #[test]
    fn non_test_run_with_s3_storage_is_allowed() {
        let settings = settings_with("s3", false);
        assert_no_remote_storage(&settings).expect("production must be allowed to use s3");
    }

    #[test]
    fn test_run_with_memory_storage_is_allowed() {
        let mut settings = settings_with("memory", true);
        assert_no_remote_storage(&settings).expect("memory storage is hermetic");
        settings.storage.r2.bucket = "halation-photos".to_string();
        assert_no_remote_storage(&settings).expect("only the backend matters");
    }

    #[test]
    fn backend_matching_is_case_insensitive() {
        let settings = settings_with("S3", true);
        assert!(
            assert_no_remote_storage(&settings).is_err(),
            "`S3` must be treated the same as `s3`"
        );
    }

    #[test]
    fn sqlite_file_path_parsing_handles_paths_uris_and_memory() {
        let mem = DatabaseSettings {
            path: ":memory:".to_string(),
            max_connections: None,
        };
        assert!(mem.is_in_memory());
        assert_eq!(None, mem.file_path());

        let file = DatabaseSettings {
            path: "/data/halation.db".to_string(),
            max_connections: Some(5),
        };
        assert!(!file.is_in_memory());
        assert_eq!(Some(Path::new("/data/halation.db")), file.file_path());

        let uri = DatabaseSettings {
            path: "sqlite:///data/halation.db?mode=rwc".to_string(),
            max_connections: None,
        };
        assert_eq!(Some(Path::new("/data/halation.db")), uri.file_path());
    }

    #[test]
    fn production_refuses_dev_signing_key_and_short_keys() {
        let mut settings = settings_with("s3", false);
        settings.secrets.session_signing_key = SecretString::new(
            "dev-only-key-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        );
        assert!(assert_production_ready(&settings, "production").is_err());
        assert!(assert_production_ready(&settings, "local").is_ok());

        settings.secrets.session_signing_key = SecretString::new("too-short".into());
        assert!(assert_production_ready(&settings, "production").is_err());

        settings.secrets.session_signing_key = SecretString::new("a".repeat(64).into());
        assert!(assert_production_ready(&settings, "production").is_ok());
    }
}
