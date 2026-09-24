// src/routes/media_rotate.rs

// dependencies
use crate::database::DatabaseBackend;
use crate::services::media;
use crate::template::TemplateRenderer;
use actix_identity::Identity;
use actix_web::{Either, Error, HttpResponse, web};
use datastar::actix::Sse;
use datastar::prelude::{ElementPatchMode, PatchElements};
use uuid::Uuid;

/// POST /fragments/media/{media_id}/rotate — owner-only: rotates the photo
/// 90° clockwise (stacking on any EXIF orientation), regenerates all
/// derivatives from the stored original, bumps the media version (cache
/// buster), and returns the re-rendered media block.
pub async fn rotate_media(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    storage: web::Data<Box<dyn crate::storage::StorageBackend>>,
    path: web::Path<Uuid>,
    identity: Option<Identity>,
) -> Result<Either<HttpResponse, Sse>, Error> {
    let Some(identity) = identity else {
        return Ok(Either::Left(HttpResponse::Unauthorized().finish()));
    };
    let Ok(user_id) = identity.id().map_err(crate::utils::e500) else {
        return Err(crate::utils::e500("unreadable identity"));
    };
    let Ok(user_id) = Uuid::parse_str(&user_id).map_err(crate::utils::e500) else {
        return Err(crate::utils::e500("unreadable identity"));
    };
    let media_id = path.into_inner();

    let info = db
        .media_for_rotation(media_id)
        .await
        .map_err(crate::utils::e500)?;

    let Some(info) = info else {
        return Ok(Either::Left(HttpResponse::NotFound().finish()));
    };
    if info.owner_id != user_id {
        return Ok(Either::Left(HttpResponse::Forbidden().finish()));
    }

    // Re-derive from the stored original: EXIF orientation applies first,
    // then the cumulative manual rotation.
    let raw = storage
        .find(&info.storage_key)
        .await
        .map_err(crate::utils::e500)?;
    let exif_orientation = media::exif_orientation(&raw);
    let (mut img, _mime) = media::decode_image(&raw).map_err(crate::utils::e500)?;
    img.apply_orientation(exif_orientation);

    let new_rotation = (info.rotation + 90) % 360;
    img = media::rotate_cw(&img, (new_rotation as u32) / 90);

    let derivatives =
        media::store_derivatives(storage.get_ref().as_ref(), info.owner_id, media_id, &img)
            .await
            .map_err(crate::utils::e500)?;

    // One store call bumps the version and persists the regenerated
    // derivatives — dimensions, sizes, and storage keys — so the database
    // stays the single source of truth for where the bytes live.
    db.apply_rotation(
        media_id,
        new_rotation,
        img.width(),
        img.height(),
        &derivatives,
    )
    .await
    .map_err(crate::utils::e500)?;

    // Re-render the media block (with the bumped version as cache buster)
    // and replace it in place via Datastar.
    let page = db
        .load_media_block_for_media(media_id)
        .await
        .map_err(crate::utils::e500)?;
    let media = page.ok_or_else(|| crate::utils::e500("rotated media has no post"))?;

    let block = serde_json::json!({ "media": media, "can_rotate": true });
    let block_html = templates.render("partials/post_media_block.html", &block)?;
    Ok(Either::Right(Sse::from(
        PatchElements::new(block_html)
            .selector("#post-media")
            .mode(ElementPatchMode::Inner)
            .into_datastar_event(),
    )))
}
