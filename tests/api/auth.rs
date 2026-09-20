// tests/api/auth.rs

use crate::helpers::spawn_app;
use uuid::Uuid;

fn register_form<'a>(username: &'a str, email: &'a str, password: &'a str) -> [(&'a str, &'a str); 3] {
    [
        ("username", username),
        ("email", email),
        ("password", password),
    ]
}

fn login_form<'a>(identifier: &'a str, password: &'a str) -> [(&'a str, &'a str); 2] {
    [("identifier", identifier), ("password", password)]
}

#[tokio::test]
async fn login_page_renders_fresh() {
    // Arrange
    let app = spawn_app().await;

    // Act
    let response = app
        .api_client
        .get(&format!("{}/login", &app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — a bare GET (no form data in context) must render, not 500
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains("Welcome back"));
}

#[tokio::test]
async fn login_page_renders_the_registration_notice() {
    // Arrange
    let app = spawn_app().await;

    // Act
    let response = app
        .api_client
        .get(&format!("{}/login?registered=1", &app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains("Account created"));
}

#[tokio::test]
async fn register_page_renders_fresh() {
    // Arrange
    let app = spawn_app().await;

    // Act
    let response = app
        .api_client
        .get(&format!("{}/register", &app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — bare GET must render without form data in context
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains("Create your account"));
}

#[tokio::test]
async fn register_creates_account_and_redirects_to_login() {
    // Arrange
    let app = spawn_app().await;

    // Act
    let response = app
        .api_client
        .post(&format!("{}/register", &app.address))
        .form(&register_form("jeff", "jeff@example.com", "hunter2hunter2"))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(303, response.status().as_u16());
    assert_eq!(
        Some("/login?registered=1"),
        response
            .headers()
            .get("Location")
            .and_then(|v| v.to_str().ok())
    );

    let stored: (String, String) = sqlx::query_as(
        "SELECT username::text, email::text FROM users WHERE username = 'jeff'",
    )
    .fetch_one(&app.db_pool)
    .await
    .expect("user should be persisted");
    assert_eq!("jeff", stored.0);
    assert_eq!("jeff@example.com", stored.1);
}

#[tokio::test]
async fn register_with_duplicate_username_conflicts() {
    // Arrange
    let app = spawn_app().await;
    let first = app
        .api_client
        .post(&format!("{}/register", &app.address))
        .form(&register_form("jeff", "jeff@example.com", "hunter2hunter2"))
        .send()
        .await
        .expect("first registration should succeed");
    assert_eq!(303, first.status().as_u16());

    // Act — same username, different email
    let response = app
        .api_client
        .post(&format!("{}/register", &app.address))
        .form(&register_form("jeff", "other@example.com", "hunter2hunter2"))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(409, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains("taken"), "the 409 page names the problem");
}

#[tokio::test]
async fn register_with_invalid_input_unprocesses() {
    // Arrange
    let app = spawn_app().await;

    // Act — bad username shape, bad email, short password
    let response = app
        .api_client
        .post(&format!("{}/register", &app.address))
        .form(&register_form("Bad Name!", "nope", "short"))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(422, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains("lowercase letters"));
    assert!(body.contains("valid email"));
    assert!(body.contains("at least 8 characters"));
}

#[tokio::test]
async fn login_sets_session_and_home_page_greets_logged_in_user() {
    // Arrange
    let app = spawn_app().await;
    app.api_client
        .post(&format!("{}/register", &app.address))
        .form(&register_form("jeff", "jeff@example.com", "hunter2hunter2"))
        .send()
        .await
        .expect("registration should succeed");

    // Act
    let response = app
        .api_client
        .post(&format!("{}/login", &app.address))
        .form(&login_form("jeff", "hunter2hunter2"))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — login redirects home and sets a session cookie
    assert_eq!(303, response.status().as_u16());
    assert_eq!(Some("/"), response.headers().get("Location").and_then(|v| v.to_str().ok()));
    let cookie = response
        .headers()
        .get_all("Set-Cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(cookie.contains("id="), "a session cookie is issued");

    // The session is server-side: exactly one row, owned by jeff
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&app.db_pool)
        .await
        .unwrap();
    assert_eq!(1, count, "the session record lives in Postgres");
    let owner: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM sessions LIMIT 1").fetch_one(&app.db_pool).await.unwrap();
    assert!(owner.is_some(), "the session row is tied to its user");

    // The home page greets the authenticated user (sees the logout button)
    let home = app
        .api_client
        .get(&format!("{}/", &app.address))
        .header("Cookie", cookie)
        .send()
        .await
        .expect("Failed to execute request.");
    assert_eq!(200, home.status().as_u16());
    let body = home.text().await.unwrap();
    assert!(
        body.contains("Log out"),
        "authenticated home shows logout; body was:\n{body}"
    );
}

#[tokio::test]
async fn login_rejects_wrong_password_without_revealing_which_half_failed() {
    // Arrange
    let app = spawn_app().await;
    app.api_client
        .post(&format!("{}/register", &app.address))
        .form(&register_form("jeff", "jeff@example.com", "hunter2hunter2"))
        .send()
        .await
        .expect("registration should succeed");

    // Act
    let response = app
        .api_client
        .post(&format!("{}/login", &app.address))
        .form(&login_form("jeff", "wrong-password"))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(401, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains("Invalid username or password"));
}

#[tokio::test]
async fn logout_destroys_the_server_side_session() {
    // Arrange — register + login, capture the cookie
    let app = spawn_app().await;
    app.api_client
        .post(&format!("{}/register", &app.address))
        .form(&register_form("jeff", "jeff@example.com", "hunter2hunter2"))
        .send()
        .await
        .expect("registration should succeed");
    let login = app
        .api_client
        .post(&format!("{}/login", &app.address))
        .form(&login_form("jeff", "hunter2hunter2"))
        .send()
        .await
        .expect("login should succeed");
    let cookie = login
        .headers()
        .get_all("Set-Cookie")
        .iter()
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("; ");

    // Act — log out with the session cookie
    let response = app
        .api_client
        .post(&format!("{}/logout", &app.address))
        .header("Cookie", cookie.clone())
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — redirect, and the server-side session is gone
    assert_eq!(303, response.status().as_u16());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&app.db_pool)
        .await
        .unwrap();
    assert_eq!(0, count, "purge must delete the session record");

    // The old cookie is dead: the home page no longer sees a logged-in user
    let home = app
        .api_client
        .get(&format!("{}/", &app.address))
        .header("Cookie", cookie)
        .send()
        .await
        .expect("Failed to execute request.");
    let body = home.text().await.unwrap();
    assert!(body.contains("Log in"), "a dead session is logged out");
    assert!(!body.contains("Log out"), "no logout button for the dead session");
}

#[tokio::test]
async fn login_is_rate_limited_per_identifier() {
    // Arrange — flux-limiter GCRA: burst 5 admits 6 consecutive attempts,
    // denial lands on the 7th.
    let app = spawn_app().await;
    let password = "not-the-password";

    // Act — burn the burst: 6 failed attempts
    for _ in 0..6 {
        let response = app
            .api_client
            .post(&format!("{}/login", &app.address))
            .form(&login_form("bruteforce", password))
            .send()
            .await
            .expect("Failed to execute request.");
        assert_eq!(401, response.status().as_u16());
    }

    // Assert — the 7th attempt is denied with 429 + Retry-After
    let response = app
        .api_client
        .post(&format!("{}/login", &app.address))
        .form(&login_form("bruteforce", password))
        .send()
        .await
        .expect("Failed to execute request.");
    assert_eq!(429, response.status().as_u16());
    assert!(response.headers().get("Retry-After").is_some());
}
