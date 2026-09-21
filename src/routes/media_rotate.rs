// src/routes/media_rotate.rs

// dependencies
use crate::services::media;
use crate::template::TemplateRenderer;
use actix_identity::Identity;
use actix_web::{Error, HttpResponse, Either, web};
use datastar::actix::Sse;
use datastar::prelude::{ElementPatchMode, PatchElements};
use uuid::Uuid;

/// POST /fragments/media/{media_id}/rotate — owner-only: rotates the photo
/// 90° clockwise (stacking on any EXIF orientation), regenerates all
/// derivatives from the stored original, bumps the media version (cache
/// buster), and returns the re-rendered media block.
pub async fn rotate_media(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    pool: web::Data<sqlx::PgPool>,
    storage: web::Data<Box<dyn crate::storage::StorageBackend>>,
    path: web::Path<Uuid>,
    identity: Option<Identity>,
) -> Result<Either<HttpResponse, Sse>, Error> {
    let Some(identity) = identity else {
        return Ok(Either::Left(HttpResponse::Unauthorized().finish()));
    };
    let Ok(user_id) = identity.id().map_err(|e| crate::utils::e500(e)) else {
        return Err(crate::utils::e500("unreadable identity"));
    };
    let Ok(user_id) = Uuid::parse_str(&user_id).map_err(|e| crate::utils::e500(e)) else {
        return Err(crate::utils::e500("unreadable identity"));
    };
    let media_id = path.into_inner();

    let row: Option<(Uuid, String, i32, i32)> = sqlx::query_as(
        "SELECT owner_id, storage_key, rotation, version FROM media WHERE id = $1",
    )
    .bind(media_id)
    .fetch_optional(pool.get_ref())
    .await
    .map_err(|e| crate::utils::e500(e))?;

    let Some((owner_id, storage_key, rotation, version)) = row else {
        return Ok(Either::Left(HttpResponse::NotFound().finish()));
    };
    if owner_id != user_id {
        return Ok(Either::Left(HttpResponse::Forbidden().finish()));
    }

    // Re-derive from the stored original: EXIF orientation applies first,
    // then the cumulative manual rotation.
    let raw = storage
        .find(&storage_key)
        .await
        .map_err(|e| crate::utils::e500(e))?;
    let exif_orientation = media::exif_orientation(&raw);
    let (mut img, _mime) = media::decode_image(&raw).map_err(|e| crate::utils::e500(e))?;
    img.apply_orientation(exif_orientation);

    let new_rotation = (rotation + 90) % 360;
    img = media::rotate_cw(&img, (new_rotation as u32) / 90);

    let derivatives =
        media::store_derivatives(storage.get_ref().as_ref(), media_id, &img)
            .await
            .map_err(|e| crate::utils::e500(e))?;
    let new_version = version + 1;

    sqlx::query(
        "UPDATE media SET rotation = $2, version = $3, width = $4, height = $5 WHERE id = $1",
    )
    .bind(media_id)
    .bind(new_rotation)
    .bind(new_version)
    .bind(img.width() as i32)
    .bind(img.height() as i32)
    .execute(pool.get_ref())
    .await
    .map_err(|e| crate::utils::e500(e))?;

    for derivative in &derivatives {
        sqlx::query(
            "UPDATE media_derivatives SET width = $3, height = $4, size_bytes = $5
             WHERE media_id = $1 AND variant = $2",
        )
        .bind(media_id)
        .bind(derivative.variant)
        .bind(derivative.width as i32)
        .bind(derivative.height as i32)
        .bind(derivative.size_bytes)
        .execute(pool.get_ref())
        .await
        .map_err(|e| crate::utils::e500(e))?;
    }

    // Re-render the media block (with the bumped version as cache buster)
    // and replace it in place via Datastar.
    let page = crate::posts::load_post_media_block_for_media(pool.get_ref(), media_id)
        .await
        .map_err(|e| crate::utils::e500(e))?;
    let media = page.ok_or_else(|| crate::utils::e500("rotated media has no post"))?;

    let block = serde_json::json!({ "media": media });
    let block_html = templates.render("partials/post_media_block.html", &block)?;
    Ok(Either::Right(Sse::from(
        PatchElements::new(block_html)
            .selector("#post-media")
            .mode(ElementPatchMode::Inner)
            .into_datastar_event(),
    )))
}
