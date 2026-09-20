// src/startup.rs

// dependencies
use crate::configuration::{DatabaseSettings, Settings};
use crate::routes::{api, health_check, pages};
use crate::storage::{InMemoryStorageBackend, OpendalStorageBackend, StorageBackend};
use crate::template::{TemplateRenderer, tera::TeraRenderer};
use actix_files::Files as ActixFiles;
use actix_web::dev::Server;
use actix_web::{App, HttpServer, web};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::net::TcpListener;
use tracing_actix_web::TracingLogger;

pub struct Application {
    port: u16,
    server: Server,
}

impl Application {
    pub async fn build(configuration: Settings) -> Result<Self, anyhow::Error> {
        let connection_pool = get_connection_pool(&configuration.database);
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

async fn run(
    listener: TcpListener,
    db_pool: PgPool,
    configuration: Settings,
) -> Result<Server, anyhow::Error> {
    let db_pool = web::Data::new(db_pool);

    let template_renderer: Box<dyn TemplateRenderer> = Box::new(TeraRenderer::new()?);
    let template_renderer = web::Data::new(template_renderer);

    let storage_backend: Box<dyn StorageBackend> = match configuration.storage.backend.as_str() {
        "s3" => Box::new(OpendalStorageBackend::new(&configuration.storage)?),
        _ => Box::new(InMemoryStorageBackend::new()),
    };
    let storage_backend = web::Data::new(storage_backend);

    let server = HttpServer::new(move || {
        App::new()
            .wrap(TracingLogger::default())
            // Liveness lives at the root, outside any future session/auth
            // middleware: it must stay the cheapest request the server can
            // answer. Dependency checks live at /api/v1/health.
            .route("/health_check", web::get().to(health_check))
            .service(
                web::scope("/api")
                    .service(web::scope("/v1").route("/health", web::get().to(api::deep_health))),
            )
            .route("/", web::get().to(pages::get_index_page))
            .service(ActixFiles::new("/static", "static").prefer_utf8(true))
            .default_service(web::to(pages::not_found))
            .app_data(db_pool.clone())
            .app_data(template_renderer.clone())
            .app_data(storage_backend.clone())
    })
    .listen(listener)?
    .run();

    Ok(server)
}
