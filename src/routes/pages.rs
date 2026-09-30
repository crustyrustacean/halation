// src/routes/pages.rs

// dependencies
use crate::template::TemplateRenderer;
use actix_identity::Identity;
use actix_web::{Error, HttpResponse, web::Data};
use serde::Serialize;

#[derive(Serialize)]
struct NotFoundContext {
    title: &'static str,
    header: &'static str,
    sub_header: &'static str,
    logged_in: bool,
}

/// default service — render the styled 404 page.
#[tracing::instrument(skip_all, name = "handler::not_found")]
pub async fn not_found(
    templates: Data<Box<dyn TemplateRenderer>>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let context = NotFoundContext {
        title: "Not found",
        header: "404",
        sub_header: "That page doesn't exist.",
        logged_in: identity.is_some(),
    };

    let body = templates.render("error.html", &serde_json::to_value(&context)?)?;
    Ok(HttpResponse::NotFound()
        .content_type("text/html")
        .body(body))
}
