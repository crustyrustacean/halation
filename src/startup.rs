// src/startup.rs

// dependencies
use crate::authentication::SqliteSessionStore;
use crate::configuration::{DatabaseSettings, Settings};
use crate::database::DatabaseBackend;
use crate::guards::require_datastar_request_header;
use crate::routes::upload::UPLOAD_LIMIT_BYTES;
use crate::routes::{api, auth, feed, health_check, media_serving, pages, profile, social, upload};
use crate::services::RateLimiter;
use crate::services::geocode::Geocoder;
use crate::storage::StorageBackend;
use crate::telemetry::request_id;
use crate::template::TemplateRenderer;
use actix_files::Files as ActixFiles;
use actix_identity::IdentityMiddleware;
use actix_session::SessionMiddleware;
use actix_session::config::PersistentSession;
use actix_web::cookie::Key;
use actix_web::dev::Server;
use actix_web::middleware::from_fn;
use actix_web::{App, HttpResponse, HttpServer, web};
use secrecy::ExposeSecret;
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;
use std::net::TcpListener;
use std::sync::Arc;
use tracing_actix_web::TracingLogger;

/// Login/register rate limiting: 5 attempts per 15 minutes, burst 5.
pub const AUTH_RATE_PER_SECOND: f64 = 5.0 / (15.0 * 60.0);
pub const AUTH_BURST_CAPACITY: f64 = 5.0;

/// Sliding session lifetime: refreshed on every state change; idle
/// sessions die after a week.
const SESSION_TTL_DAYS: i64 = 7;

/// Injected application dependencies. Constructed in `main.rs` (or the test
/// harness) and passed into [`Application::build`] so the HTTP server stays
/// decoupled from concrete infrastructure implementations.
#[derive(Clone)]
pub struct AppDeps {
    pub template_renderer: Arc<dyn TemplateRenderer>,
    pub database: Arc<dyn DatabaseBackend>,
    pub storage: Arc<dyn StorageBackend>,
    pub session_store: SqliteSessionStore,
    pub rate_limiter: Arc<RateLimiter>,
    pub geocoder: Arc<Geocoder>,
}

pub struct Application {
    port: u16,
    server: Server,
    #[allow(dead_code)]
    deps: AppDeps,
}

impl Application {
    pub async fn build(configuration: Settings, deps: AppDeps) -> Result<Self, anyhow::Error> {
        // Before anything with a side effect: a test run must never reach
        // real object storage, whatever the surrounding environment says.
        if let Err(why) = crate::configuration::assert_no_remote_storage(&configuration) {
            return Err(anyhow::anyhow!(why));
        }

        // Refuse to boot in production with the checked-in dev signing key or
        // an invalid SQLite path/URI.
        let env_name = crate::configuration::current_environment();
        if let Err(why) = crate::configuration::assert_production_ready(&configuration, &env_name) {
            return Err(anyhow::anyhow!(why));
        }

        let address = format!(
            "{}:{}",
            configuration.application.host, configuration.application.port
        );
        let listener = TcpListener::bind(address)?;
        let port = listener.local_addr()?.port();
        let server = run(listener, configuration, deps.clone()).await?;
        Ok(Self { port, server, deps })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn run_until_stopped(self) -> Result<(), std::io::Error> {
        self.server.await
    }
}

pub fn get_connection_pool(configuration: &DatabaseSettings) -> SqlitePool {
    let max_connections = if configuration.is_in_memory() {
        1
    } else {
        configuration.max_connections.unwrap_or(5)
    };
    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_lazy_with(configuration.connect_options())
}

async fn run(
    listener: TcpListener,
    configuration: Settings,
    deps: AppDeps,
) -> Result<Server, anyhow::Error> {
    let database = web::Data::from(deps.database);
    let template_renderer = web::Data::from(deps.template_renderer);
    let storage_backend = web::Data::from(deps.storage);
    let rate_limiter = web::Data::from(deps.rate_limiter);
    let geocoder = web::Data::from(deps.geocoder);
    let registration_open = web::Data::new(configuration.application.registration_open);
    let trusted_proxy = web::Data::new(crate::configuration::TrustedProxy(
        configuration.application.trusted_proxy,
    ));
    let static_dir = configuration.application.static_dir;

    // SessionMiddleware contains Rc internals and must be constructed per
    // worker inside the closure; the pool-backed store and the signing key
    // bytes are Send + Clone, so they cross the boundary instead.
    let session_store = deps.session_store;
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
            .wrap(from_fn(request_id))
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
            .service(
                ActixFiles::new("/static", static_dir.clone())
                    .prefer_utf8(true)
                    .default_handler(web::to(HttpResponse::NotFound)),
            )
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
