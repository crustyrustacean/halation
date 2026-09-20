// src/routes/upload.rs

// dependencies
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
    pool: web::Data<sqlx::PgPool>,
    storage: web::Data<Box<dyn crate::storage::StorageBackend>>,
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
        Ok::<_, Error>(HttpResponse::build(status)
            .content_type("text/html")
            .body(templates.render("upload.html", &context)?))
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

    // Pipeline: process each file, then persist media rows + the post in a
    // single transaction so a half-uploaded post can never exist.
    let mut tx = pool.begin().await.map_err(|e| crate::utils::e500(e))?;

    let post_id: Uuid = sqlx::query_scalar(
        "INSERT INTO posts (user_id, caption) VALUES ($1, $2) RETURNING id",
    )
    .bind(user_id)
    .bind(&caption)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| crate::utils::e500(e))?;

    for (position, file) in form.files.iter().enumerate() {
        let raw = std::fs::read(file.file.path())
            .map_err(|e| crate::utils::e500(format!("temp file vanished: {e}")))?;

        let stored = match media::process_and_store(storage.get_ref().as_ref(), &raw).await {
            Ok(stored) => stored,
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
        };

        let media_id = media::insert_media(&mut *tx, user_id, &stored, None)
            .await
            .map_err(|e| crate::utils::e500(e))?;

        sqlx::query(
            "INSERT INTO post_media (post_id, media_id, position) VALUES ($1, $2, $3)",
        )
        .bind(post_id)
        .bind(media_id)
        .bind(position as i32)
        .execute(&mut *tx)
        .await
        .map_err(|e| crate::utils::e500(e))?;
    }

    tx.commit().await.map_err(|e| crate::utils::e500(e))?;

    Ok(HttpResponse::SeeOther()
        .insert_header(("Location", format!("/p/{post_id}")))
        .finish())
}

#[derive(Debug, serde::Serialize)]
pub struct PostMediaView {
    pub media_id: Uuid,
}

#[derive(Debug, serde::Serialize)]
pub struct PostPageContext {
    pub title: String,
    pub header: &'static str,
    pub sub_header: &'static str,
    pub logged_in: bool,
    pub post_id: Uuid,
    pub caption: Option<String>,
    pub username: String,
    pub created_at: String,
    pub media: Vec<PostMediaView>,
}

/// GET /p/{post_id} — the post permalink: photo set + caption + owner.
pub async fn get_post_page(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    pool: web::Data<sqlx::PgPool>,
    path: web::Path<Uuid>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let post_id = path.into_inner();

    let row: Option<(Uuid, Option<String>, String, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT p.id, p.caption, u.username::text, p.created_at
             FROM posts p JOIN users u ON u.id = p.user_id
             WHERE p.id = $1",
        )
        .bind(post_id)
        .fetch_optional(pool.get_ref())
        .await
        .map_err(|e| crate::utils::e500(e))?;

    let Some((id, caption, username, created_at)) = row else {
        return Ok(pages_not_found(&templates, identity).await);
    };

    let media_ids: Vec<(Uuid, i32)> = sqlx::query_as(
        "SELECT media_id, position FROM post_media WHERE post_id = $1 ORDER BY position",
    )
    .bind(post_id)
    .fetch_all(pool.get_ref())
    .await
    .map_err(|e| crate::utils::e500(e))?;

    let context = PostPageContext {
        title: "Post".into(),
        header: "Halation",
        sub_header: "A post.",
        logged_in: identity.is_some(),
        post_id: id,
        caption,
        username,
        created_at: created_at.format("%b %e, %Y").to_string(),
        media: media_ids
            .into_iter()
            .map(|(media_id, _)| PostMediaView { media_id })
            .collect(),
    };

    let body = templates.render("post.html", &serde_json::to_value(&context)?)?;
    Ok(HttpResponse::Ok().content_type("text/html").body(body))
}

async fn pages_not_found(
    templates: &web::Data<Box<dyn TemplateRenderer>>,
    identity: Option<Identity>,
) -> HttpResponse {
    let context = serde_json::json!({
        "title": "Not found",
        "header": "404",
        "sub_header": "That post doesn't exist (or isn't yours to see).",
        "logged_in": identity.is_some(),
    });

    match templates.render("error.html", &context) {
        Ok(body) => HttpResponse::NotFound().content_type("text/html").body(body),
        Err(_) => HttpResponse::NotFound().finish(),
    }
}
