// tests/api/registration_closed.rs

use crate::helpers::{spawn_app, spawn_app_with};

fn register_form(username: &str, email: &str, password: &str) -> [(&'static str, String); 3] {
    [
        ("username", username.to_string()),
        ("email", email.to_string()),
        ("password", password.to_string()),
    ]
}

/// With registration open (the default), the page renders the form and
/// the nav carries the Register link.
#[tokio::test]
async fn registration_open_renders_form_and_nav_link() {
    // Arrange
    let app = spawn_app().await;

    // Act
    let response = app
        .api_client
        .get(format!("{}/register", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.expect("Expected an HTML body");
    assert!(body.contains("Create your account"), "form renders");
    assert!(body.contains("href=\"/register\""), "nav Register link");

    // Act — the login page carries the sign-up nudge while open
    let response = app
        .api_client
        .get(format!("{}/login", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.expect("Expected an HTML body");
    assert!(
        body.contains("New here?"),
        "login page sign-up nudge while open"
    );
}

/// Registration closed: GET /register lands on the closed page, and every
/// page hides the Register nav link and the sign-up nudge.
#[tokio::test]
async fn closed_registration_redirects_and_hides_links() {
    // Arrange
    let app = spawn_app_with(Box::new(|c| {
        c.application.registration_open = false;
    }))
    .await;

    // Act — direct GET
    let response = app
        .api_client
        .get(format!("{}/register", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — 303 to the closed page
    assert_eq!(303, response.status().as_u16());
    assert_eq!(
        "/register/closed",
        response
            .headers()
            .get("Location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
    );

    // Act — the closed page itself
    let response = app
        .api_client
        .get(format!("{}/register/closed", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.expect("Expected an HTML body");
    assert!(
        body.contains("Registration is closed"),
        "the closed page states its purpose"
    );
    assert!(
        !body.contains("href=\"/register\""),
        "no Register nav link while closed"
    );

    // Act — login page under closed registration
    let response = app
        .api_client
        .get(format!("{}/login", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.expect("Expected an HTML body");
    assert!(
        !body.contains("New here?"),
        "sign-up nudge hidden while closed"
    );
}

/// Registration closed: POST /register refuses without creating an
/// account, and the same credentials still work via the seeded path —
/// existing users are unaffected.
#[tokio::test]
async fn closed_registration_refuses_posts_without_creating_accounts() {
    // Arrange
    let app = spawn_app_with(Box::new(|c| {
        c.application.registration_open = false;
    }))
    .await;

    // Act — POST the form straight at the endpoint
    let response = app
        .api_client
        .post(format!("{}/register", app.address))
        .form(&register_form(
            "intruder",
            "intruder@example.com",
            "hunter2hunter2",
        ))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — refused, redirected to the closed page
    assert_eq!(303, response.status().as_u16());
    assert_eq!(
        "/register/closed",
        response
            .headers()
            .get("Location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
    );

    // Assert — no account was created: login with those credentials fails
    let response = app
        .api_client
        .post(format!("{}/login", app.address))
        .form(&[("identifier", "intruder"), ("password", "hunter2hunter2")])
        .send()
        .await
        .expect("Failed to execute request.");
    assert_eq!(
        401,
        response.status().as_u16(),
        "credentials must not exist after a refused registration"
    );

    // Assert — and the users table has no such row (belt and braces)
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username = 'intruder'")
        .fetch_one(&app.db_pool)
        .await
        .unwrap();
    assert_eq!(0, count, "no user row for the refused registration");
}

/// Actix keys `app_data` by the *type* of the stored value, so two
/// independent `web::Data<bool>` registrations are indistinguishable — the
/// second silently replaces the first and handlers still asking for
/// `Data<bool>` receive the wrong value.
///
/// Registering `trusted_proxy` as a bare `bool` did exactly that: it
/// overwrote `registration_open`, and every `/register` request started
/// reading `trusted_proxy` (false), 303-ing to `/register/closed` with
/// registration fully open.
///
/// The fix is the `TrustedProxy` newtype — a distinct `TypeId`. This test
/// pins the collision from both directions: each flag must still be
/// readable independently, and neither may leak into the other.
#[tokio::test]
async fn registration_flag_and_trusted_proxy_flag_do_not_collide() {
    // Arrange — registration closed, trusted proxy on. These are the two
    // configurations that are indistinguishable as a bare `bool`.
    let app = spawn_app_with(Box::new(|c| {
        c.application.registration_open = false;
        c.application.trusted_proxy = true;
    }))
    .await;

    // Act
    let register = app
        .api_client
        .get(format!("{}/register", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — `registration_open=false` is what must drive this, and the
    // fact that `trusted_proxy=true` is set is what would have leaked.
    assert_eq!(
        303,
        register.status().as_u16(),
        "registration_open must still read false when trusted_proxy is true"
    );

    // And the reverse pairing: both true must leave registration open.
    let app = spawn_app_with(Box::new(|c| {
        c.application.registration_open = true;
        c.application.trusted_proxy = true;
    }))
    .await;
    let register = app
        .api_client
        .get(format!("{}/register", app.address))
        .send()
        .await
        .expect("Failed to execute request.");
    assert_eq!(
        200,
        register.status().as_u16(),
        "trusted_proxy=true must not be mistaken for registration_open"
    );
}
