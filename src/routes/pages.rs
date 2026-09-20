// src/routes/pages.rs

// dependencies
use crate::template::TemplateRenderer;
use actix_identity::Identity;
use actix_web::{Error, HttpResponse, web::Data};
use serde::Serialize;

#[derive(Serialize)]
struct PageContext {
    title: &'static str,
    header: &'static str,
    sub_header: &'static str,
    logged_in: bool,
}

/// GET / — placeholder home page. The chronological feed lands with the
/// posts phase; the template shell is already the real one.
pub async fn get_index_page(
    templates: Data<Box<dyn TemplateRenderer>>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let context = PageContext {
        title: "Home",
        header: "Halation",
        sub_header: "The golden glow around the highlights.",
        logged_in: identity.is_some(),
    };

    let body = templates.render("index.html", &serde_json::to_value(&context)?)?;
    Ok(HttpResponse::Ok().content_type("text/html").body(body))
}

/// default service — render the styled 404 page.
pub async fn not_found(
    templates: Data<Box<dyn TemplateRenderer>>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let context = PageContext {
        title: "Not found",
        header: "404",
        sub_header: "That page doesn't exist.",
        logged_in: identity.is_some(),
    };

    let body = templates.render("error.html", &serde_json::to_value(&context)?)?;
    Ok(HttpResponse::NotFound().content_type("text/html").body(body))
}
