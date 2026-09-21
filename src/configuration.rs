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
        assert_eq!("https://abc123.r2.cloudflarestorage.com", settings.storage.r2.endpoint);
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
    }
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
