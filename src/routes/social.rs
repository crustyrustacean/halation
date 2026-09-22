// src/routes/social.rs

// dependencies
use crate::follows;
use crate::template::TemplateRenderer;
use actix_identity::Identity;
use actix_web::{Error, HttpResponse, Either, web};
use datastar::actix::Sse;
use datastar::prelude::{ElementPatchMode, PatchElements};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

fn viewer_id(identity: Option<&Identity>) -> Result<Option<Uuid>, Error> {
    match identity {
        Some(identity) => {
            let raw = identity.id().map_err(|e| crate::utils::e500(e))?;
            let id = Uuid::parse_str(&raw).map_err(|e| crate::utils::e500(e))?;
            Ok(Some(id))
        }
        None => Ok(None),
    }
}

#[derive(Serialize)]
struct FollowButtonContext {
    username: String,
    is_following: bool,
}

fn follow_button(
    templates: &dyn TemplateRenderer,
    username: &str,
    is_following: bool,
) -> Result<String, Error> {
    let context = serde_json::json!({ "username": username, "is_following": is_following });
    templates
        .render("partials/follow_button.html", &context)
        .map_err(|e| crate::utils::e500(e))
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
pub async fn follow_user(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    pool: web::Data<PgPool>,
    path: web::Path<String>,
    identity: Option<Identity>,
) -> Result<Either<HttpResponse, Sse>, Error> {
    let Some(viewer) = viewer_id(identity.as_ref())? else {
        return Ok(Either::Left(HttpResponse::Unauthorized().finish()));
    };
    let username = path.into_inner();

    let Some(target) = follows::find_user_id_by_username(pool.get_ref(), &username)
        .await
        .map_err(|e| crate::utils::e500(e))?
    else {
        return Ok(Either::Left(HttpResponse::NotFound().finish()));
    };
    if target == viewer {
        return Ok(Either::Left(
            HttpResponse::BadRequest().body("You cannot follow yourself."),
        ));
    }

    follows::follow(pool.get_ref(), viewer, target)
        .await
        .map_err(|e| crate::utils::e500(e))?;

    let html = follow_button(templates.get_ref().as_ref(), &username, true)?;
    Ok(Either::Right(button_swap(html)))
}

/// DELETE /fragments/users/{username}/follow — unfollow the account.
pub async fn unfollow_user(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    pool: web::Data<PgPool>,
    path: web::Path<String>,
    identity: Option<Identity>,
) -> Result<Either<HttpResponse, Sse>, Error> {
    let Some(viewer) = viewer_id(identity.as_ref())? else {
        return Ok(Either::Left(HttpResponse::Unauthorized().finish()));
    };
    let username = path.into_inner();

    let Some(target) = follows::find_user_id_by_username(pool.get_ref(), &username)
        .await
        .map_err(|e| crate::utils::e500(e))?
    else {
        return Ok(Either::Left(HttpResponse::NotFound().finish()));
    };

    follows::unfollow(pool.get_ref(), viewer, target)
        .await
        .map_err(|e| crate::utils::e500(e))?;

    let html = follow_button(templates.get_ref().as_ref(), &username, false)?;
    Ok(Either::Right(button_swap(html)))
}

#[derive(Serialize)]
struct SocialListContext {
    title: String,
    heading: String,
    logged_in: bool,
    accounts: Vec<follows::AccountSummary>,
}

async fn render_social_list(
    templates: &dyn TemplateRenderer,
    pool: &PgPool,
    username: &str,
    kind: &str,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    // Unknown account → styled 404
    if follows::find_user_id_by_username(pool, username)
        .await
        .map_err(|e| crate::utils::e500(e))?
        .is_none()
    {
        let context = serde_json::json!({
            "title": "Not found",
            "header": "404",
            "sub_header": "That account doesn't exist.",
            "logged_in": identity.is_some(),
        });
        let body = templates.render("error.html", &context)?;
        return Ok(HttpResponse::NotFound().content_type("text/html").body(body));
    }

    let accounts = match kind {
        "followers" => follows::followers_of(pool, username).await,
        _ => follows::following_of(pool, username).await,
    }
    .map_err(|e| crate::utils::e500(e))?;

    let label = if kind == "followers" { "Followers" } else { "Following" };
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
pub async fn get_followers_page(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    pool: web::Data<PgPool>,
    path: web::Path<String>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let username = path.into_inner();
    render_social_list(templates.get_ref().as_ref(), pool.get_ref(), &username, "followers", identity).await
}

/// GET /u/{username}/following
pub async fn get_following_page(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    pool: web::Data<PgPool>,
    path: web::Path<String>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let username = path.into_inner();
    render_social_list(templates.get_ref().as_ref(), pool.get_ref(), &username, "following", identity).await
}
