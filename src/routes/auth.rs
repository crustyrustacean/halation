// src/routes/auth.rs

// dependencies
use crate::authentication::{NewUser, UserStoreError, hash_password, verify_password};
use crate::configuration::TrustedProxy;
use crate::database::DatabaseBackend;
use crate::services::RateLimiter;
use crate::template::TemplateRenderer;
use actix_identity::Identity;
use actix_session::SessionExt;
use actix_web::{Error, HttpMessage, HttpRequest, HttpResponse, web};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// context + render helpers
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct AuthPageContext {
    title: &'static str,
    header: &'static str,
    sub_header: &'static str,
    logged_in: bool,
    errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
}

fn render_page(
    templates: &dyn TemplateRenderer,
    template: &str,
    context: &serde_json::Value,
) -> Result<String, Error> {
    Ok(templates.render(template, context)?)
}

fn html(status: actix_web::http::StatusCode, body: String) -> HttpResponse {
    HttpResponse::build(status)
        .content_type("text/html")
        .body(body)
}

/// The address to key rate limiting on.
///
/// Behind a reverse proxy the socket peer is always the proxy, so
/// `peer_addr()` alone gives one shared bucket for every visitor. That is
/// what this did until it was fixed: the login limiter keyed on Caddy's
/// address, making it a global throttle that unlimited attempts from any
/// source would trip while protecting nothing.
///
/// `X-Forwarded-For` is only consulted when `application.trusted_proxy` is
/// set, and only its **last** entry is used.
///
/// That direction is deliberate, and the opposite of what the header looks
/// like it wants. A proxy appends the address it observed to the end of the
/// chain, so with exactly one trusted hop the last entry is the one the
/// proxy itself saw. Taking the *first* entry — which is what actix's own
/// `realip_remote_addr` does — takes the leftmost value, which is the one a
/// client controls and can set to anything.
///
/// With no trusted proxy configured there is nothing to vouch for the
/// header, so it is ignored entirely and the socket address is used. An
/// unset value therefore cannot be turned into a spoofable one.
///
/// Unparseable values fall back to the socket address rather than becoming
/// a shared literal like `"unknown"`, which would collapse every malformed
/// header into one bucket — the same failure mode as the original bug.
fn client_ip(req: &HttpRequest, trusted_proxy: bool) -> String {
    let socket = req
        .peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();

    let from_socket = || {
        if socket.is_empty() {
            "unknown".to_string()
        } else {
            socket.clone()
        }
    };

    if !trusted_proxy {
        return from_socket();
    }

    req.headers()
        .get("X-Forwarded-For")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next_back())
        .map(str::trim)
        .and_then(|v| {
            v.trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .map(|ip| ip.to_string())
        .unwrap_or_else(from_socket)
}

/// Normalize and validate registration input. Returns the stored,
/// lowercase forms on success or user-friendly error strings for the form.
fn validate_registration(
    username: &str,
    email: &str,
    password: &str,
) -> Result<(String, String), Vec<String>> {
    let mut errors = Vec::new();

    let username = username.trim().to_lowercase();
    if !(3..=30).contains(&username.len()) {
        errors.push("Username must be 3-30 characters.".into());
    } else if !username
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        errors.push("Username may only contain lowercase letters, digits, and underscores.".into());
    }

    let email = email.trim().to_lowercase();
    let email_ok = {
        let parts: Vec<&str> = email.split('@').collect();
        match parts.as_slice() {
            [local, domain] => !local.is_empty() && domain.contains('.') && !domain.contains(' '),
            _ => false,
        }
    };
    if !email_ok {
        errors.push("Enter a valid email address.".into());
    }

    if password.len() < 8 {
        errors.push("Password must be at least 8 characters.".into());
    }

    if errors.is_empty() {
        Ok((username, email))
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// registration
// ---------------------------------------------------------------------------

/// GET /register/closed — the "registration is closed" page. Served for
/// logged-in visitors too (direct navigation is harmless and the page
/// links back to /login).
///
/// `identity` is skipped: actix-identity's `Identity` has no `Debug` impl,
/// and logging the caller's id is not useful here.
#[tracing::instrument(skip_all, name = "handler::registration_closed")]
pub async fn get_registration_closed_page(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    let context = AuthPageContext {
        title: "Registration closed",
        header: "Halation",
        sub_header: "Registration is closed.",
        logged_in: identity.is_some(),
        errors: Vec::new(),
        notice: None,
        username: None,
        email: None,
    };

    let body = render_page(
        templates.get_ref().as_ref(),
        "registration_closed.html",
        &serde_json::to_value(&context)?,
    )?;
    Ok(html(actix_web::http::StatusCode::OK, body))
}

// `Debug` is hand-written so the password can never be printed. A
// `#[derive(Debug)]` here is a live hazard: `#[tracing::instrument]` and
// `dbg!` both record arguments via `Debug`, and either would write the
// plaintext password to the log. The `password` field is simply omitted.
#[derive(Deserialize)]
pub struct RegisterFormData {
    pub username: String,
    pub email: String,
    pub password: String,
}

impl std::fmt::Debug for RegisterFormData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisterFormData")
            .field("username", &self.username)
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

#[tracing::instrument(
    skip_all,
    name = "handler::register_page",
    fields(registration_open = ?registration_open)
)]
pub async fn get_register_page(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    registration_open: web::Data<bool>,
    identity: Option<Identity>,
) -> Result<HttpResponse, Error> {
    if !**registration_open {
        return Ok(HttpResponse::SeeOther()
            .insert_header(("Location", "/register/closed"))
            .finish());
    }

    let context = AuthPageContext {
        title: "Register",
        header: "Halation",
        sub_header: "Create your account.",
        logged_in: identity.is_some(),
        errors: Vec::new(),
        notice: None,
        username: None,
        email: None,
    };

    let body = render_page(
        templates.get_ref().as_ref(),
        "register.html",
        &serde_json::to_value(&context)?,
    )?;
    Ok(html(actix_web::http::StatusCode::OK, body))
}

/// `form` is skipped on purpose: `RegisterFormData` derives `Debug`, and
/// `instrument` records arguments with `Debug` by default — recording it
/// verbatim would write the plaintext password to the log. Only the
/// normalized, non-secret parts are recorded, via the `fields(...)` below.
#[tracing::instrument(
    skip_all,
    name = "handler::register",
    fields(registration_open = ?registration_open, identifier = tracing::field::Empty)
)]
pub async fn post_register(
    req: HttpRequest,
    form: web::Form<RegisterFormData>,
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    rate_limiter: web::Data<RateLimiter>,
    registration_open: web::Data<bool>,
    trusted_proxy: web::Data<TrustedProxy>,
) -> Result<HttpResponse, Error> {
    // Registration closed: refuse before burning rate-limit budget or
    // touching the database. A 303 to the closed page keeps no-JS
    // browsers on a sensible page instead of a dead form.
    if !**registration_open {
        return Ok(HttpResponse::SeeOther()
            .insert_header(("Location", "/register/closed"))
            .finish());
    }

    // Rate limit per IP: 5 per 15 minutes, burst 5
    let decision = rate_limiter.check(&format!("register:{}", client_ip(&req, trusted_proxy.0)));
    if !decision.allowed {
        let body = render_page(
            templates.get_ref().as_ref(),
            "register.html",
            &serde_json::json!({
                "title": "Register", "header": "Halation",
                "sub_header": "Create your account.", "logged_in": false,
                "errors": ["Too many attempts. Try again shortly."],
            }),
        )?;
        return Ok(
            HttpResponse::build(actix_web::http::StatusCode::TOO_MANY_REQUESTS)
                .insert_header(("Retry-After", decision.retry_after_seconds.to_string()))
                .content_type("text/html")
                .body(body),
        );
    }

    let context = |errors: Vec<String>| {
        serde_json::json!({
            "title": "Register", "header": "Halation",
            "sub_header": "Create your account.", "logged_in": false,
            "errors": errors,
            "username": form.username, "email": form.email,
        })
    };

    // Validate (normalizes username/email to their stored, lowercase form)
    let (username, email) = match validate_registration(&form.username, &form.email, &form.password)
    {
        Ok(pair) => pair,
        Err(errors) => {
            let body = render_page(
                templates.get_ref().as_ref(),
                "register.html",
                &context(errors),
            )?;
            return Ok(html(
                actix_web::http::StatusCode::UNPROCESSABLE_ENTITY,
                body,
            ));
        }
    };

    let password_hash = hash_password(&form.password).map_err(crate::utils::e500)?;

    match db
        .insert_user(NewUser {
            username,
            email,
            password_hash,
        })
        .await
    {
        Ok(_) => Ok(HttpResponse::SeeOther()
            .insert_header(("Location", "/login?registered=1"))
            .finish()),
        Err(UserStoreError::DuplicateUsername) => {
            let body = render_page(
                templates.get_ref().as_ref(),
                "register.html",
                &context(vec!["That username is already taken.".into()]),
            )?;
            Ok(html(actix_web::http::StatusCode::CONFLICT, body))
        }
        Err(UserStoreError::DuplicateEmail) => {
            let body = render_page(
                templates.get_ref().as_ref(),
                "register.html",
                &context(vec!["That email is already registered.".into()]),
            )?;
            Ok(html(actix_web::http::StatusCode::CONFLICT, body))
        }
        Err(e) => Err(crate::utils::e500(e)),
    }
}

// ---------------------------------------------------------------------------
// login / logout
// ---------------------------------------------------------------------------

// See `RegisterFormData`: same reasoning, and the same hand-written
// `Debug` that refuses to render the password.
#[derive(Deserialize)]
pub struct LoginFormData {
    pub identifier: String,
    pub password: String,
}

impl std::fmt::Debug for LoginFormData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginFormData")
            .field("identifier", &self.identifier)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    pub registered: Option<String>,
}

#[tracing::instrument(skip_all, name = "handler::login_page")]
pub async fn get_login_page(
    templates: web::Data<Box<dyn TemplateRenderer>>,
    identity: Option<Identity>,
    query: web::Query<LoginQuery>,
) -> Result<HttpResponse, Error> {
    let notice = if query.registered.is_some() {
        Some("Account created. Log in to continue.")
    } else {
        None
    };

    let context = AuthPageContext {
        title: "Log in",
        header: "Halation",
        sub_header: "Welcome back.",
        logged_in: identity.is_some(),
        errors: Vec::new(),
        notice,
        username: None,
        email: None,
    };

    let body = render_page(
        templates.get_ref().as_ref(),
        "login.html",
        &serde_json::to_value(&context)?,
    )?;
    Ok(html(actix_web::http::StatusCode::OK, body))
}

/// `form` is skipped deliberately: it carries the plaintext password, and
/// `Debug`-based argument recording would leak it into the log. The
/// identifier is recorded by hand below, after normalization.
#[tracing::instrument(
    skip_all,
    name = "handler::login",
    fields(identifier = tracing::field::Empty)
)]
pub async fn post_login(
    req: HttpRequest,
    form: web::Form<LoginFormData>,
    templates: web::Data<Box<dyn TemplateRenderer>>,
    db: web::Data<Box<dyn DatabaseBackend>>,
    rate_limiter: web::Data<RateLimiter>,
    trusted_proxy: web::Data<TrustedProxy>,
) -> Result<HttpResponse, Error> {
    let ip = client_ip(&req, trusted_proxy.0);
    let identifier = form.identifier.trim().to_lowercase();
    // The identifier is a username or email — account-identifying, but not
    // a secret, and it is the single most useful field for diagnosing a
    // failed login. The password never appears.
    tracing::Span::current().record("identifier", tracing::field::display(&identifier));

    // Rate limit per IP + identifier: 5 per 15 minutes, burst 5
    let decision = rate_limiter.check(&format!("login:{ip}:{identifier}"));
    if !decision.allowed {
        let body = render_page(
            templates.get_ref().as_ref(),
            "login.html",
            &serde_json::json!({
                "title": "Log in", "header": "Halation",
                "sub_header": "Welcome back.", "logged_in": false,
                "errors": ["Too many attempts. Try again shortly."],
                "username": form.identifier,
            }),
        )?;
        return Ok(
            HttpResponse::build(actix_web::http::StatusCode::TOO_MANY_REQUESTS)
                .insert_header(("Retry-After", decision.retry_after_seconds.to_string()))
                .content_type("text/html")
                .body(body),
        );
    }

    let login_context = |errors: Vec<String>| {
        serde_json::json!({
            "title": "Log in", "header": "Halation",
            "sub_header": "Welcome back.", "logged_in": false,
            "errors": errors,
            "username": form.identifier,
        })
    };

    // One generic error for both unknown identifier and bad password —
    // the form never reveals which half failed.
    let failure = || async {
        let body = render_page(
            templates.get_ref().as_ref(),
            "login.html",
            &login_context(vec!["Invalid username or password.".into()]),
        )
        .expect("rendering the login page cannot fail for context reasons");
        html(actix_web::http::StatusCode::UNAUTHORIZED, body)
    };

    let user = match db.find_user_by_identifier(&identifier).await {
        Ok(Some(user)) => user,
        Ok(None) => return Ok(failure().await),
        Err(e) => return Err(crate::utils::e500(e)),
    };

    let password_ok =
        verify_password(&form.password, &user.password_hash).map_err(crate::utils::e500)?;
    if !password_ok {
        return Ok(failure().await);
    }

    // Establish identity (actix-identity stores it inside the session) and
    // plant the metadata our SessionStore lifts into queryable columns.
    Identity::login(&req.extensions(), user.id.to_string()).map_err(crate::utils::e500)?;

    let session = req.get_session();
    session
        .insert("user_id", user.id.to_string())
        .map_err(crate::utils::e500)?;
    if let Some(ua) = req.headers().get(actix_web::http::header::USER_AGENT) {
        session
            .insert("login_user_agent", ua.to_str().unwrap_or_default())
            .map_err(crate::utils::e500)?;
    }
    if let Some(ip) = req.peer_addr() {
        session
            .insert("login_ip", ip.to_string())
            .map_err(crate::utils::e500)?;
    }

    Ok(HttpResponse::SeeOther()
        .insert_header(("Location", "/"))
        .finish())
}

#[tracing::instrument(skip_all, name = "handler::logout")]
pub async fn post_logout(req: HttpRequest, identity: Option<Identity>) -> HttpResponse {
    if let Some(identity) = identity {
        identity.logout();
    }
    // purge() removes the session record server-side, not just the cookie.
    req.get_session().purge();
    HttpResponse::SeeOther()
        .insert_header(("Location", "/login"))
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parsing half of `client_ip`, exercised directly. The socket side
    /// needs a real `HttpRequest`, so these tests pin the part that carries
    /// the security argument: which end of `X-Forwarded-For` is trusted, and
    /// what happens to a header nobody should be able to set.
    mod forwarded_for {
        /// The last entry, mirroring `client_ip`'s `next_back()`.
        fn last_entry(header: &str) -> Option<&str> {
            header.split(',').next_back().map(str::trim)
        }

        #[test]
        fn the_last_entry_is_the_proxy_observed_address() {
            // A proxy appends what it saw, so the tail is the trustworthy end.
            assert_eq!(last_entry("203.0.113.9"), Some("203.0.113.9"));
            assert_eq!(
                last_entry("1.1.1.1, 2.2.2.2, 203.0.113.9"),
                Some("203.0.113.9")
            );
        }

        #[test]
        fn a_client_supplied_prefix_is_not_where_we_look() {
            // This is the whole reason `client_ip` does NOT use the first
            // entry, which is what actix's `realip_remote_addr` takes.
            let forged = "6.6.6.6, 203.0.113.9";
            assert_eq!(forged.split(',').next().map(str::trim), Some("6.6.6.6"));
            assert_eq!(last_entry(forged), Some("203.0.113.9"));
        }

        #[test]
        fn ipv6_and_bracketed_forms_parse() {
            assert!("2001:db8::1".parse::<std::net::IpAddr>().is_ok());
            assert!(
                "[2001:db8::1]"
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .is_ok()
            );
        }

        #[test]
        fn a_garbage_header_yields_none_rather_than_a_shared_bucket() {
            // Returning "unknown" for every malformed header would collapse
            // them into one rate-limit key — the original bug, reintroduced.
            for bad in ["not-an-ip", "999.999.999.999", "", "   "] {
                assert_eq!(
                    bad.trim().parse::<std::net::IpAddr>().ok(),
                    None,
                    "{bad:?} should not parse as an address"
                );
            }
        }
    }

    #[test]
    fn valid_registration_passes_and_normalizes() {
        // Act
        let result = validate_registration(" Jeff ", "Jeff@Example.COM", "hunter2hunter2");

        // Assert
        let (username, email) = result.expect("should validate");
        assert_eq!(username, "jeff");
        assert_eq!(email, "jeff@example.com");
    }

    #[test]
    fn invalid_inputs_collect_all_errors() {
        // Act
        let result = validate_registration("Bad Name!", "not-an-email", "short");

        // Assert
        let errors = result.expect_err("should fail");
        assert_eq!(errors.len(), 3, "one error per invalid field");
    }
}
