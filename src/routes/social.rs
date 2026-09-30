// src/routes/social.rs

// dependencies
use crate::database::DatabaseBackend;
use crate::template::TemplateRenderer;
use actix_identity::Identity;
use actix_web::{Either, Error, HttpResponse, web};
use datastar::actix::Sse;
use datastar::prelude::{ElementPatchMode, PatchElements};
use uuid::Uuid;

fn viewer_id(identity: Option<&Identity>) -> Result<Option<Uuid>, Error> {
    match identity {
        Some(identity) => {
            let raw = identity.id().map_err(crate::utils::e500)?;
            let id = Uuid::parse_str(&raw).map_err(crate::utils::e500)?;
            Ok(Some(id))
        }
        None => Ok(None),
    }
}

fn follow_button(
    templates: &dyn TemplateRenderer,
    username: &str,
    is_following: bool,
) -> Result<String, Error> {
    let context = serde_json::json!({ "username": username, "is_following": is_following });
    templates
        .render("partials/follow_button.html", &context)
        .map_err(crate::utils::e500)
}

fn button_swap(html: String) -> Sse {
    Sse::from(
        PatchElements::new(html)
            .selector("#follow-btn")
            .mode(ElementPatchMode::Replace)
            .into_datastar_event(),
    )
}

/// POST /fragments/users/{username}/follow — follow the account.
#[tracing::instrument(
    skip_all,
    name = "handler::follow",
    fields(target = tracing::field::Empty)
)]
pub async fn follow_user(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    path: web::Path<String>,
    identity: Option<Identity>,
) -> Result<Either<HttpResponse, Sse>, Error> {
    let Some(viewer) = viewer_id(identity.as_ref())? else {
        return Ok(Either::Left(HttpResponse::Unauthorized().finish()));
    };
    let username = path.into_inner();
    tracing::Span::current().record("target", tracing::field::display(&username));

    let Some(target) = db
        .find_user_id_by_username(&username)
        .await
        .map_err(crate::utils::e500)?
    else {
        return Ok(Either::Left(HttpResponse::NotFound().finish()));
    };
    if target == viewer {
        return Ok(Either::Left(
            HttpResponse::BadRequest().body("You cannot follow yourself."),
        ));
    }

    db.follow(viewer, target)
        .await
        .map_err(crate::utils::e500)?;

    let html = follow_button(templates.get_ref().as_ref(), &username, true)?;
    Ok(Either::Right(button_swap(html)))
}

/// DELETE /fragments/users/{username}/follow — unfollow the account.
#[tracing::instrument(
    skip_all,
    name = "handler::unfollow",
    fields(target = tracing::field::Empty)
)]
pub async fn unfollow_user(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    path: web::Path<String>,
    identity: Option<Identity>,
) -> Result<Either<HttpResponse, Sse>, Error> {
    let Some(viewer) = viewer_id(identity.as_ref())? else {
        return Ok(Either::Left(HttpResponse::Unauthorized().finish()));
    };
    let username = path.into_inner();
    tracing::Span::current().record("target", tracing::field::display(&username));

    let Some(target) = db
        .find_user_id_by_username(&username)
        .await
        .map_err(crate::utils::e500)?
    else {
        return Ok(Either::Left(HttpResponse::NotFound().finish()));
    };

    db.unfollow(viewer, target)
        .await
        .map_err(crate::utils::e500)?;

    let html = follow_button(templates.get_ref().as_ref(), &username, false)?;
    Ok(Either::Right(button_swap(html)))
}

async fn render_social_list(
    templates: &dyn TemplateRenderer,
    db: &dyn DatabaseBackend,
    username: &str,
    kind: &str,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    // Unknown account → styled 404
    if db
        .find_user_id_by_username(username)
        .await
        .map_err(crate::utils::e500)?
        .is_none()
    {
        let context = serde_json::json!({
            "title": "Not found",
            "header": "404",
            "sub_header": "That account doesn't exist.",
            "logged_in": identity.is_some(),
        });
        let body = templates.render("error.html", &context)?;
        return Ok(HttpResponse::NotFound()
            .content_type("text/html")
            .body(body));
    }

    let accounts = match kind {
        "followers" => db.followers_of(username).await,
        _ => db.following_of(username).await,
    }
    .map_err(crate::utils::e500)?;

    let label = if kind == "followers" {
        "Followers"
    } else {
        "Following"
    };
    let context = serde_json::json!({
        "title": format!("{label} — @{username}"),
        "heading": format!("{label} — @{username}"),
        "logged_in": identity.is_some(),
        "accounts": accounts,
    });

    let body = templates.render("social_list.html", &context)?;
    Ok(HttpResponse::Ok().content_type("text/html").body(body))
}

/// GET /u/{username}/followers
#[tracing::instrument(
    skip_all,
    name = "handler::followers_page",
    fields(username = tracing::field::Empty)
)]
pub async fn get_followers_page(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    path: web::Path<String>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let username = path.into_inner();
    tracing::Span::current().record("username", tracing::field::display(&username));
    render_social_list(
        templates.get_ref().as_ref(),
        db.get_ref().as_ref(),
        &username,
        "followers",
        identity,
    )
    .await
}

/// GET /u/{username}/following
#[tracing::instrument(
    skip_all,
    name = "handler::following_page",
    fields(username = tracing::field::Empty)
)]
pub async fn get_following_page(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    path: web::Path<String>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let username = path.into_inner();
    tracing::Span::current().record("username", tracing::field::display(&username));
    render_social_list(
        templates.get_ref().as_ref(),
        db.get_ref().as_ref(),
        &username,
        "following",
        identity,
    )
    .await
}
