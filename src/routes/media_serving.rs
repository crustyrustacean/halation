// src/routes/media_serving.rs

// dependencies
use crate::storage::StorageBackend;
use actix_web::{HttpRequest, HttpResponse, http::header, web};
use uuid::Uuid;

const PUBLIC_VARIANTS: [&str; 3] = ["thumb", "medium", "large"];
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// GET /media/{media_id}/{variant} — public derivative serving.
///
/// Only processed variants are public; the original stays owner-private
/// (it carries EXIF, including GPS). Long immutable cache headers: keys
/// are content-addressed by media id + variant, so they never change.
pub async fn get_media_derivative(
    pool: web::Data<sqlx::PgPool>,
    storage: web::Data<Box<dyn StorageBackend>>,
    path: web::Path<(Uuid, String)>,
    _req: HttpRequest,
) -> HttpResponse {
    let (media_id, variant) = path.into_inner();

    if !PUBLIC_VARIANTS.contains(&variant.as_str()) {
        return HttpResponse::NotFound().finish();
    }

    let storage_key: Option<String> = sqlx::query_scalar(
        "SELECT storage_key FROM media_derivatives WHERE media_id = $1 AND variant = $2",
    )
    .bind(media_id)
    .bind(&variant)
    .fetch_optional(pool.get_ref())
    .await
    .ok()
    .flatten();

    let Some(storage_key) = storage_key else {
        return HttpResponse::NotFound().finish();
    };

    match storage.find(&storage_key).await {
        Ok(bytes) => HttpResponse::Ok()
            .content_type("image/jpeg")
            .insert_header((header::CACHE_CONTROL, CACHE_IMMUTABLE))
            .body(bytes),
        Err(crate::storage::StorageError::NotFound(_)) => HttpResponse::NotFound().finish(),
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}
