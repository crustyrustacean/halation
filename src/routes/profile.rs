// src/routes/profile.rs

// dependencies
use crate::database::DatabaseBackend;
use crate::posts;
use crate::template::TemplateRenderer;
use actix_identity::Identity;
use actix_web::{Error, HttpResponse, web};
use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize)]
struct ProfilePageContext {
    title: String,
    header: &'static str,
    sub_header: String,
    logged_in: bool,
    profile: posts::ProfileView,
}

/// GET /u/{username} — the public profile: identity card + post grid.
/// Backed by `ProfileView`, the read-only type that structurally cannot
/// carry an email or password hash.
#[tracing::instrument(
    skip_all,
    name = "handler::profile",
    fields(username = tracing::field::Empty)
)]
pub async fn get_profile(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    path: web::Path<String>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let username = path.into_inner();
    tracing::Span::current().record("username", tracing::field::display(&username));
    let viewer = identity
        .as_ref()
        .and_then(|i| i.id().ok())
        .and_then(|s| Uuid::parse_str(&s).ok());

    let profile = db
        .load_profile(&username, viewer)
        .await
        .map_err(crate::utils::e500)?;
    match profile {
        Some(profile) => {
            let sub_header = match &profile.bio {
                Some(bio) => bio.clone(),
                None => format!("Posts by @{}", profile.username),
            };
            let context = ProfilePageContext {
                title: format!("@{}", profile.username),
                header: "Halation",
                sub_header,
                logged_in: identity.is_some(),
                profile,
            };

            let body = templates.render("profile.html", &serde_json::to_value(&context)?)?;
            Ok(HttpResponse::Ok().content_type("text/html").body(body))
        }
        None => {
            let context = serde_json::json!({
                "title": "Not found",
                "header": "404",
                "sub_header": "That account doesn't exist.",
                "logged_in": identity.is_some(),
            });
            let body = templates.render("error.html", &context)?;
            Ok(HttpResponse::NotFound()
                .content_type("text/html")
                .body(body))
        }
    }
}
