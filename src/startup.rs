// src/startup.rs

// dependencies
use crate::authentication::PostgresSessionStore;
use crate::configuration::{DatabaseSettings, Settings};
use crate::database::{DatabaseBackend, PostgresDatabase};
use crate::guards::require_datastar_request_header;
use crate::routes::upload::UPLOAD_LIMIT_BYTES;
use crate::routes::{
    api, auth, feed, health_check, media_rotate, media_serving, pages, profile, social, upload,
};
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
                     container running? (scripts/init_dev_db.sh starts it.)\n\t{e}",
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
            sqlx::query(&format!(r#"CREATE DATABASE "{}""#, settings.database_name))
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

    let template_renderer: web::Data<Box<dyn TemplateRenderer>> =
        web::Data::new(Box::new(TeraRenderer::new()?));

    let storage: Box<dyn StorageBackend> = match configuration.storage.backend.as_str() {
        "s3" => Box::new(OpendalStorageBackend::new(&configuration.storage)?),
        _ => Box::new(InMemoryStorageBackend::new()),
    };
    let storage_backend = web::Data::new(storage);

    let rate_limiter = web::Data::new(RateLimiter::new(AUTH_RATE_PER_SECOND, AUTH_BURST_CAPACITY)?);
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
            .route("/login", web::get().to(auth::get_login_page))
            .route("/login", web::post().to(auth::post_login))
            .route("/logout", web::post().to(auth::post_logout))
            // MVP composer + post permalink (Phase 2)
            .route("/upload", web::get().to(upload::get_upload_page))
            .route("/upload", web::post().to(upload::post_upload))
            .route("/p/{post_id}", web::get().to(upload::get_post_page))
            .route(
                "/fragments/media/{media_id}/rotate",
                web::post().to(media_rotate::rotate_media),
            )
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
