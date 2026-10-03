// src/startup.rs

// dependencies
use crate::authentication::PostgresSessionStore;
use crate::configuration::{DatabaseSettings, Settings};
use crate::database::{DatabaseBackend, PostgresDatabase};
use crate::guards::require_datastar_request_header;
use crate::routes::upload::UPLOAD_LIMIT_BYTES;
use crate::routes::{api, auth, feed, health_check, media_serving, pages, profile, social, upload};
use crate::services::RateLimiter;
use crate::services::geocode::Geocoder;
use crate::storage::{InMemoryStorageBackend, OpendalStorageBackend, StorageBackend};
use crate::template::{TemplateRenderer, tera::TeraRenderer};
use actix_files::Files as ActixFiles;
use actix_identity::IdentityMiddleware;
use actix_session::SessionMiddleware;
use actix_session::config::PersistentSession;
use actix_web::cookie::Key;
use actix_web::dev::Server;
use actix_web::middleware::from_fn;
use actix_web::{App, HttpServer, web};
use anyhow::Context as _;
use secrecy::ExposeSecret;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgPool};
use std::net::TcpListener;
use tracing_actix_web::TracingLogger;

/// Login/register rate limiting: 5 attempts per 15 minutes, burst 5.
const AUTH_RATE_PER_SECOND: f64 = 5.0 / (15.0 * 60.0);
const AUTH_BURST_CAPACITY: f64 = 5.0;

/// Sliding session lifetime: refreshed on every state change; idle
/// sessions die after a week.
const SESSION_TTL_DAYS: i64 = 7;

pub struct Application {
    port: u16,
    server: Server,
}

impl Application {
    pub async fn build(configuration: Settings) -> Result<Self, anyhow::Error> {
        // Before anything with a side effect: a test run must never reach
        // real object storage, whatever the surrounding environment says.
        if let Err(why) = crate::configuration::assert_no_remote_storage(&configuration) {
            return Err(anyhow::anyhow!(why));
        }

        // Fail fast and self-heal: create the database if it doesn't exist
        // and apply migrations, so `cargo run` works on a fresh machine.
        ensure_database_exists(&configuration.database).await?;

        let connection_pool = get_connection_pool(&configuration.database);
        sqlx::migrate!("./migrations")
            .run(&connection_pool)
            .await
            .context("Failed to run database migrations")?;
        tracing::info!("database ready; migrations up to date");

        let address = format!(
            "{}:{}",
            configuration.application.host, configuration.application.port
        );
        let listener = TcpListener::bind(address)?;
        let port = listener.local_addr()?.port();
        let server = run(listener, connection_pool, configuration).await?;
        Ok(Self { port, server })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn run_until_stopped(self) -> Result<(), std::io::Error> {
        self.server.await
    }
}

pub fn get_connection_pool(configuration: &DatabaseSettings) -> PgPool {
    PgPoolOptions::new().connect_lazy_with(configuration.connect_options())
}

/// Create the configured database if it does not exist yet. A connection
/// failure that is *not* "database does not exist" (SQLSTATE 3D000) is
/// surfaced with actionable context — e.g. Postgres not running at all.
async fn ensure_database_exists(settings: &DatabaseSettings) -> Result<(), anyhow::Error> {
    match sqlx::PgConnection::connect_with(&settings.connect_options()).await {
        // Database exists and accepts our credentials.
        Ok(conn) => {
            conn.close()
                .await
                .context("Failed to close probe connection")?;
            Ok(())
        }
        Err(e) => {
            let missing = matches!(&e, sqlx::Error::Database(db)
                if db.code().as_deref() == Some("3D000"));

            if !missing {
                return Err(anyhow::anyhow!(
                    "Failed to connect to Postgres at {}:{} as {} — is the dev \
                     container running? (cargo xtask dev-db starts it.)\n\t{e}",
                    settings.host,
                    settings.port,
                    settings.username
                ));
            }

            // Create the database through the maintenance connection.
            let maintenance = DatabaseSettings {
                database_name: "postgres".to_string(),
                ..settings.clone()
            };
            let mut conn = sqlx::PgConnection::connect_with(&maintenance.connect_options())
                .await
                .context("Failed to connect to the Postgres maintenance database")?;
            // `CREATE DATABASE` is a utility statement and cannot take bind
            // parameters, so the name has to be interpolated. It is validated
            // as a plain identifier first (see `validate_database_name`), so
            // this assertion is backed by a real check rather than trust.
            let name = validate_database_name(&settings.database_name)?;
            sqlx::query(sqlx::AssertSqlSafe(format!(r#"CREATE DATABASE "{name}""#)))
                .execute(&mut conn)
                .await
                .context("Failed to create the application database")?;
            tracing::info!("created database {}", settings.database_name);
            Ok(())
        }
    }
}

async fn run(
    listener: TcpListener,
    db_pool: PgPool,
    configuration: Settings,
) -> Result<Server, anyhow::Error> {
    // Routes see the domain trait; the raw pool stays an implementation
    // detail of the Postgres backend and the session store.
    let database: Box<dyn DatabaseBackend> = Box::new(PostgresDatabase::new(db_pool.clone()));
    let database = web::Data::new(database);

    let template_renderer: web::Data<Box<dyn TemplateRenderer>> = {
        // Site-wide settings injected into every render context — the nav
        // Register link and the login page's sign-up nudge read these.
        let mut site = serde_json::Map::new();
        site.insert(
            "registration_open".into(),
            serde_json::json!(configuration.application.registration_open),
        );
        web::Data::new(Box::new(TeraRenderer::with_site(site)?) as Box<dyn TemplateRenderer>)
    };

    let storage: Box<dyn StorageBackend> = match configuration.storage.backend.as_str() {
        "s3" => Box::new(OpendalStorageBackend::new(&configuration.storage)?),
        _ => Box::new(InMemoryStorageBackend::new()),
    };
    let storage_backend = web::Data::new(storage);

    let rate_limiter = web::Data::new(RateLimiter::new(AUTH_RATE_PER_SECOND, AUTH_BURST_CAPACITY)?);
    let registration_open = web::Data::new(configuration.application.registration_open);
    let trusted_proxy = web::Data::new(crate::configuration::TrustedProxy(
        configuration.application.trusted_proxy,
    ));
    let geocoder = web::Data::new(Geocoder::new(
        configuration.geocode.enabled,
        configuration.geocode.base_url.clone(),
    ));

    // SessionMiddleware contains Rc internals and must be constructed per
    // worker inside the closure; the pool-backed store and the signing key
    // bytes are Send + Clone, so they cross the boundary instead.
    let session_store = PostgresSessionStore::new(db_pool.clone());
    let secret_key_bytes = configuration
        .secrets
        .session_signing_key
        .expose_secret()
        .as_bytes()
        .to_vec();

    let server = HttpServer::new(move || {
        let session_middleware =
            SessionMiddleware::builder(session_store.clone(), Key::from(&secret_key_bytes))
                .session_lifecycle(
                    PersistentSession::default()
                        .session_ttl(actix_web::cookie::time::Duration::days(SESSION_TTL_DAYS)),
                )
                .build();

        App::new()
            .wrap(TracingLogger::default())
            // Identity rides on sessions, so the session middleware sits
            // underneath it (actix wraps are onion-ordered: first = outer).
            .wrap(IdentityMiddleware::default())
            .wrap(session_middleware)
            // Liveness stays outside the session middlewares' work: it is
            // mounted here so every wrapped layer is above it in intent,
            // and its handler touches no state anyway.
            .route("/health_check", web::get().to(health_check))
            .service(
                web::scope("/api")
                    .service(web::scope("/v1").route("/health", web::get().to(api::deep_health))),
            )
            .route("/", web::get().to(feed::get_feed))
            .route("/recent", web::get().to(feed::get_recent))
            .route("/u/{username}", web::get().to(profile::get_profile))
            .route(
                "/u/{username}/followers",
                web::get().to(social::get_followers_page),
            )
            .route(
                "/u/{username}/following",
                web::get().to(social::get_following_page),
            )
            .route("/hashtags/{tag}", web::get().to(feed::get_hashtag_feed))
            // Classic form auth: full-page POSTs with SameSite=Lax protection
            .route("/register", web::get().to(auth::get_register_page))
            .route("/register", web::post().to(auth::post_register))
            .route(
                "/register/closed",
                web::get().to(auth::get_registration_closed_page),
            )
            .route("/login", web::get().to(auth::get_login_page))
            .route("/login", web::post().to(auth::post_login))
            .route("/logout", web::post().to(auth::post_logout))
            // MVP composer + post permalink (Phase 2)
            .route("/upload", web::get().to(upload::get_upload_page))
            .route("/upload", web::post().to(upload::post_upload))
            .route("/p/{post_id}", web::get().to(upload::get_post_page))
            .route(
                "/media/{media_id}/{variant}",
                web::get().to(media_serving::get_media_derivative),
            )
            // Datastar fragment endpoints live under this scope; every
            // mutation must carry the Datastar-Request header or 403.
            .service(
                web::scope("/fragments")
                    .route("/feed", web::get().to(feed::feed_fragment))
                    .route(
                        "/users/{username}/follow",
                        web::post().to(social::follow_user),
                    )
                    .route(
                        "/users/{username}/follow",
                        web::delete().to(social::unfollow_user),
                    )
                    .wrap(from_fn(require_datastar_request_header)),
            )
            .service(ActixFiles::new("/static", "static").prefer_utf8(true))
            .default_service(web::to(pages::not_found))
            .app_data(database.clone())
            .app_data(template_renderer.clone())
            .app_data(storage_backend.clone())
            .app_data(rate_limiter.clone())
            .app_data(registration_open.clone())
            .app_data(trusted_proxy.clone())
            .app_data(geocoder.clone())
            .app_data(
                actix_multipart::form::MultipartFormConfig::default()
                    .total_limit(UPLOAD_LIMIT_BYTES),
            )
    })
    .listen(listener)?
    .run();

    Ok(server)
}

/// Reject anything that is not a plain, safely quotable database name.
///
/// `CREATE DATABASE` cannot be parameterised, so the name reaches the
/// statement as text. This allowlist is the check that makes that safe:
/// lowercase letters, digits and underscores only, starting with a letter.
/// That excludes the double quote (the only character that could break out
/// of the quoted identifier), every punctuation character, and separators
/// needed for schema-qualified or multi-statement forms.
///
/// Test databases are generated as UUIDs, which arrive lowercased here;
/// an uppercase name is rejected rather than silently folded, so a
/// misconfiguration fails loudly instead of creating a surprising database.
fn validate_database_name(name: &str) -> anyhow::Result<&str> {
    let mut chars = name.chars();
    // The first character may be a letter *or a digit*: the test harness
    // names throwaway databases with random UUIDs, and roughly 5 in 8 of
    // those begin with a hex digit, so requiring a letter made
    // `build_bootstraps_a_missing_database` fail on most runs. Digits are
    // safe here because the name is always emitted inside double quotes.
    let starts_validly =
        matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit());
    // Hyphens are allowed because the test harness names throwaway
    // databases with UUIDs (`d3af3912-…`). Postgres accepts a hyphen in a
    // quoted identifier, and it is as inert as an underscore for injection
    // purposes; what matters is that the double quote is excluded.
    let rest_are_safe =
        chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');

    if starts_validly && rest_are_safe && !name.is_empty() {
        Ok(name)
    } else {
        Err(anyhow::anyhow!(
            "database name {name:?} is not a plain identifier — use lowercase \
             letters, digits, underscores and hyphens, starting with a letter \
             or digit (got it from APP_DATABASE__DATABASE_NAME or database_name \
             in config)"
        ))
    }
}

#[cfg(test)]
mod database_name_tests {
    use super::validate_database_name;

    #[test]
    fn accepts_plain_identifiers() {
        assert_eq!("halation", validate_database_name("halation").unwrap());
        assert_eq!("test_db_1", validate_database_name("test_db_1").unwrap());
        // A UUID test database, lowercased.
        let uuid = "d3af3912-bee4-45cc-86cf-f69b7f0e67cb";
        assert_eq!(uuid, validate_database_name(uuid).unwrap());
    }

    #[test]
    fn accepts_uuid_names_starting_with_a_digit() {
        // Regression: the harness picks a *random* database UUID, and
        // roughly 5 in 8 begin with a hex digit. A leading digit must be
        // accepted or `build_bootstraps_a_missing_database` fails on most
        // runs while appearing to pass on others.
        for uuid in [
            "4e56999e-e480-4d4e-b5ed-2855fd9c129d",
            "0f9a1c22-1111-4222-8333-444455556666",
            "7a3d9e01-2222-4333-8444-555566667777",
        ] {
            assert_eq!(uuid, validate_database_name(uuid).unwrap(), "{uuid}");
        }
    }

    #[test]
    fn rejects_quote_escapes_and_punctuation() {
        // The double quote is the one character that could terminate the
        // quoted identifier and append statements.
        assert!(validate_database_name(r#"h"; DROP DATABASE postgres; --"#).is_err());
        assert!(validate_database_name("h\"x").is_err());
        assert!(validate_database_name("postgres; SELECT 1").is_err());
        // Schema-qualified and wildcard forms have no business here.
        assert!(validate_database_name("public.halation").is_err());
        assert!(validate_database_name("db*").is_err());
        // A hyphen is inert for injection purposes and is what the test
        // harness's UUID database names contain, so it must be accepted.
        assert_eq!("db-name", validate_database_name("db-name").unwrap());
    }

    #[test]
    fn rejects_bad_shapes() {
        assert!(validate_database_name("").is_err(), "empty");
        assert!(validate_database_name("_db").is_err(), "leading underscore");
        assert!(validate_database_name("Halation").is_err(), "uppercase");
        assert!(validate_database_name("db name").is_err(), "space");
        assert!(validate_database_name("db\n").is_err(), "newline");
        assert!(validate_database_name("héllo").is_err(), "non-ascii");
    }
}
