// src/routes/api/health.rs

// dependencies
use crate::database::DatabaseBackend;
use crate::storage::StorageBackend;
use actix_web::{HttpResponse, web::Data};
use serde::Serialize;

#[derive(Serialize)]
struct HealthReport {
    status: &'static str,
    version: &'static str,
    database: &'static str,
    storage: &'static str,
}

/// GET /api/v1/health — deep health for monitoring: probes every dependency.
/// JSON is deliberate here: the consumers are machines, not browsers.
pub async fn deep_health(
    db: Data<Box<dyn DatabaseBackend>>,
    storage: Data<Box<dyn StorageBackend>>,
) -> HttpResponse {
    let database_ok = db.ping().await.is_ok();
    let storage_ok = storage.health_check().await.is_ok();

    let report = HealthReport {
        status: if database_ok && storage_ok {
            "ok"
        } else {
            "degraded"
        },
        version: env!("CARGO_PKG_VERSION"),
        database: if database_ok { "ok" } else { "unavailable" },
        storage: if storage_ok { "ok" } else { "unavailable" },
    };

    if database_ok && storage_ok {
        HttpResponse::Ok().json(report)
    } else {
        HttpResponse::ServiceUnavailable().json(report)
    }
}
