// src/routes/health_check.rs

// dependencies
use actix_web::HttpResponse;

/// GET /health_check — liveness for machines (load balancers, the test
/// harness). Dependency-free by design: no database, no storage, no session
/// middleware. A dependency blip must never fail a liveness probe — that
/// way lies cascade-restarts of healthy machines. Dependency health lives
/// at GET /api/v1/health.
#[tracing::instrument(skip_all, name = "handler::liveness")]
pub async fn health_check() -> HttpResponse {
    HttpResponse::Ok().finish()
}
