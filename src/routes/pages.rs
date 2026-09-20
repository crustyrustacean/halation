// src/routes/pages.rs

// dependencies
use crate::template::TemplateRenderer;
use actix_web::{HttpResponse, web::Data};
use serde::Serialize;

#[derive(Serialize)]
struct PageContext<'a> {
    title: &'a str,
    header: &'a str,
    sub_header: &'a str,
}

/// GET / — placeholder home page. The chronological feed lands with the
/// posts phase; the template shell is already the real one.
pub async fn get_index_page(
    templates: Data<Box<dyn TemplateRenderer>>,
) -> Result<HttpResponse, actix_web::Error> {
    let context = PageContext {
        title: "Home",
        header: "Halation",
        sub_header: "The golden glow around the highlights.",
    };

    let html = templates.render("index.html", &serde_json::to_value(&context)?)?;
    Ok(HttpResponse::Ok().content_type("text/html").body(html))
}

/// default service — render the styled 404 page.
pub async fn not_found(
    templates: Data<Box<dyn TemplateRenderer>>,
) -> Result<HttpResponse, actix_web::Error> {
    let context = serde_json::json!({
        "title": "Not found",
        "header": "404",
        "sub_header": "That page doesn't exist.",
    });

    let html = templates.render("error.html", &context)?;
    Ok(HttpResponse::NotFound().content_type("text/html").body(html))
}
