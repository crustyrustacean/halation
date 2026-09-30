// src/routes/upload.rs

// dependencies
use crate::database::DatabaseBackend;
use crate::services::geocode::Geocoder;
use crate::services::media::{self, MediaError};
use crate::template::TemplateRenderer;
use actix_identity::Identity;
use actix_multipart::form::{MultipartForm, tempfile::TempFile, text::Text};
use actix_web::{Error, HttpResponse, web};
use uuid::Uuid;

/// Hard cap for a single upload request (all files + form parts).
pub const UPLOAD_LIMIT_BYTES: usize = 50 * 1024 * 1024;
/// Max images per post — the mockup shows 2-up photo sets; leave headroom.
const MAX_FILES_PER_POST: usize = 4;
const MAX_CAPTION_CHARS: usize = 1000;

#[derive(Debug, MultipartForm)]
pub struct UploadForm {
    pub caption: Text<String>,
    #[multipart(rename = "files")]
    pub files: Vec<TempFile>,
}

fn redirect_login() -> HttpResponse {
    HttpResponse::SeeOther()
        .insert_header(("Location", "/login"))
        .finish()
}

pub async fn get_upload_page(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let identity = match identity {
        Some(identity) => identity,
        None => return Ok(redirect_login()),
    };

    let context = serde_json::json!({
        "title": "Upload",
        "header": "Halation",
        "sub_header": "New photo post.",
        "logged_in": true,
        "username": identity.id().unwrap_or_default(),
        "errors": [],
    });

    let body = templates.render("upload.html", &context)?;
    Ok(HttpResponse::Ok().content_type("text/html").body(body))
}

pub async fn post_upload(
    MultipartForm(form): MultipartForm<UploadForm>,
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    storage: web::Data<Box<dyn crate::storage::StorageBackend>>,
    geocoder: web::Data<Geocoder>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let identity = match identity {
        Some(identity) => identity,
        None => return Ok(redirect_login()),
    };
    let user_id: Uuid = identity
        .id()
        .ok()
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| crate::utils::e500("authenticated identity is not a uuid"))?;

    let render_error = move |status: actix_web::http::StatusCode, errors: Vec<String>| async move {
        let context = serde_json::json!({
            "title": "Upload", "header": "Halation",
            "sub_header": "New photo post.", "logged_in": true,
            "errors": errors,
        });
        Ok::<_, Error>(
            HttpResponse::build(status)
                .content_type("text/html")
                .body(templates.render("upload.html", &context)?),
        )
    };

    if form.files.is_empty() {
        return render_error(
            actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            vec!["Attach at least one photo.".into()],
        )
        .await;
    }
    if form.files.len() > MAX_FILES_PER_POST {
        return render_error(
            actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            vec![format!("A post holds at most {MAX_FILES_PER_POST} photos.")],
        )
        .await;
    }
    let caption = form.caption.trim().to_string();
    if caption.chars().count() > MAX_CAPTION_CHARS {
        return render_error(
            actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
            vec![format!("Captions cap at {MAX_CAPTION_CHARS} characters.")],
        )
        .await;
    }

    // Pipeline: process every file first (decode + store), then persist
    // the whole post — media rows, derivative rows, post links, hashtags,
    // location — in one store-side transaction, so a half-uploaded post
    // can never exist and no DB connection is held across image work.
    let mut stored_media: Vec<media::StoredMedia> = Vec::with_capacity(form.files.len());
    for (position, file) in form.files.iter().enumerate() {
        let raw = std::fs::read(file.file.path())
            .map_err(|e| crate::utils::e500(format!("temp file vanished: {e}")))?;

        match media::process_and_store(storage.get_ref().as_ref(), user_id, &raw).await {
            Ok(stored) => stored_media.push(stored),
            Err(MediaError::InvalidImage) => {
                return render_error(
                    actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
                    vec![format!(
                        "File {} isn't a supported image (JPEG, PNG, WebP, HEIC).",
                        position + 1
                    )],
                )
                .await;
            }
            Err(e) => return Err(crate::utils::e500(e)),
        }
    }

    // GPS -> location name: best-effort reverse geocode of the first
    // photo's coordinates (see services::geocode for the privacy note).
    let mut location_name: Option<String> = None;
    if geocoder.enabled() {
        for file in form.files.iter() {
            let raw = std::fs::read(file.file.path()).unwrap_or_default();
            let exif = media::extract_exif(&raw);
            let lat = exif
                .as_ref()
                .and_then(|e| e.get("gps_latitude"))
                .and_then(|v| v.as_f64());
            let lng = exif
                .as_ref()
                .and_then(|e| e.get("gps_longitude"))
                .and_then(|v| v.as_f64());
            if let (Some(lat), Some(lng)) = (lat, lng) {
                location_name = geocoder.reverse(lat, lng).await;
                break;
            }
        }
    }

    let post_id = db
        .create_post_with_media(user_id, &caption, location_name, &stored_media)
        .await
        .map_err(crate::utils::e500)?;

    Ok(HttpResponse::SeeOther()
        .insert_header(("Location", format!("/p/{post_id}")))
        .finish())
}

/// GET /p/{post_id} — the post permalink: photo set + caption + owner +
/// EXIF meta row (loaded via `DatabaseBackend::load_post_page`).
pub async fn get_post_page(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    path: web::Path<Uuid>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let post_id = path.into_inner();
    match db
        .load_post_page(post_id)
        .await
        .map_err(crate::utils::e500)?
    {
        Some(page) => {
            let mut context = serde_json::to_value(&page)?;
            context["title"] = serde_json::json!("Post");
            context["header"] = serde_json::json!("Halation");
            context["sub_header"] = serde_json::json!("A post.");
            context["logged_in"] = serde_json::json!(identity.is_some());

            let body = templates.render("post.html", &context)?;
            Ok(HttpResponse::Ok().content_type("text/html").body(body))
        }
        None => {
            let context = serde_json::json!({
                "title": "Not found",
                "header": "404",
                "sub_header": "That post doesn't exist (or isn't yours to see).",
                "logged_in": identity.is_some(),
            });
            let body = templates.render("error.html", &context)?;
            Ok(HttpResponse::NotFound()
                .content_type("text/html")
                .body(body))
        }
    }
}
