// src/guards.rs

// dependencies
use actix_web::{
    Error, http::Method, middleware::Next,
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    http::StatusCode,
};
use thiserror::Error;

/// Header every Datastar request carries; a misfired cross-site form
/// cannot set custom headers, which is what makes this a CSRF guard.
pub const DATASTAR_REQUEST: &str = "Datastar-Request";

/// Blocks cross-site form attacks against Datastar fragment endpoints:
/// mutations must present the `Datastar-Request` header or they are
/// rejected with 403.
///
/// Classic form POSTs (/register, /login, /logout) are NOT covered by this
/// guard — they rely on `SameSite=Lax` cookies and are mounted outside any
/// `/fragments` scope.
pub async fn require_datastar_request_header(
    req: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    let is_mutation = matches!(
        *req.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );

    if is_mutation && req.headers().get(DATASTAR_REQUEST).is_none() {
        return Err(CsrfError.into());
    }

    next.call(req).await
}

#[derive(Debug, Error)]
#[error("Cross-site or non-Datastar mutation rejected")]
pub struct CsrfError;

impl actix_web::ResponseError for CsrfError {
    fn status_code(&self) -> StatusCode {
        StatusCode::FORBIDDEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::middleware::from_fn;
    use actix_web::{App, HttpResponse, test, web};

    async fn probe() -> HttpResponse {
        HttpResponse::Ok().finish()
    }

    // Each test inits its own app: the wrapped App's concrete type is not
    // nameable, and test-local init keeps the middleware under test honest.
    macro_rules! app {
        () => {
            test::init_service(
                App::new()
                    .wrap(from_fn(require_datastar_request_header))
                    .route("/fragments/probe", web::post().to(probe))
                    .route("/fragments/probe", web::get().to(probe)),
            )
        };
    }

    #[tokio::test]
    async fn mutation_without_datastar_header_is_rejected() {
        // Arrange
        let app = app!().await;

        // Act — a POST with no Datastar-Request header. The guard rejects
        // via an actix Error, so call the service directly (call_service
        // would panic on the Err) and inspect the response error.
        use actix_web::dev::Service;
        let request = test::TestRequest::post()
            .uri("/fragments/probe")
            .to_request();
        let error = match app.call(request).await {
            Ok(_) => panic!("unguarded mutation must be rejected"),
            Err(error) => error,
        };

        // Assert
        assert_eq!(error.as_response_error().status_code(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn mutation_with_datastar_header_passes() {
        // Arrange
        let app = app!().await;

        // Act
        let request = test::TestRequest::post()
            .uri("/fragments/probe")
            .insert_header((DATASTAR_REQUEST, "true"))
            .to_request();
        let response = test::call_service(&app, request).await;

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn reads_without_header_pass_through() {
        // Arrange — GETs are never mutations, so they need no header
        let app = app!().await;

        // Act
        let request = test::TestRequest::get()
            .uri("/fragments/probe")
            .to_request();
        let response = test::call_service(&app, request).await;

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
    }
}
