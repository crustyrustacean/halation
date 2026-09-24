// tests/api/social.rs

use crate::helpers::{register_and_login, spawn_app};
use uuid::Uuid;

const DATASTAR: &str = "Datastar-Request";

fn follow_url(addr: &str, username: &str) -> String {
    format!("{addr}/fragments/users/{username}/follow")
}

#[tokio::test]
async fn follow_fragment_requires_auth() {
    // Arrange — target user exists; no cookie on the request
    let app = spawn_app().await;
    register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;

    // Act
    let response = app
        .api_client
        .post(follow_url(&app.address, "jeff"))
        .header(DATASTAR, "true")
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(401, response.status().as_u16());
}

#[tokio::test]
async fn follow_fragment_requires_datastar_header() {
    // Arrange
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;

    // Act — authenticated mutation without the Datastar header
    let response = app
        .api_client
        .post(follow_url(&app.address, "jeff"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — the CSRF guard rejects it
    assert_eq!(403, response.status().as_u16());
}

#[tokio::test]
async fn follow_unknown_user_is_404() {
    // Arrange
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;

    // Act
    let response = app
        .api_client
        .post(follow_url(&app.address, "nobody"))
        .header("Cookie", &cookie)
        .header(DATASTAR, "true")
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(404, response.status().as_u16());
}

#[tokio::test]
async fn self_follow_is_rejected() {
    // Arrange
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;

    // Act
    let response = app
        .api_client
        .post(follow_url(&app.address, "jeff"))
        .header("Cookie", &cookie)
        .header(DATASTAR, "true")
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(400, response.status().as_u16());
}

#[tokio::test]
async fn follow_unfollow_round_trip() {
    // Arrange
    let app = spawn_app().await;
    let _jeff = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    let alice = register_and_login(&app, "alice", "alice@example.com", "wonderland1").await;

    // Act — alice follows jeff
    let response = app
        .api_client
        .post(follow_url(&app.address, "jeff"))
        .header("Cookie", &alice)
        .header(DATASTAR, "true")
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — button flips to Following
    assert_eq!(200, response.status().as_u16());
    assert!(response.status().is_success());
    let body = response.text().await.unwrap();
    assert!(body.contains("Following"), "button flips to Following");
    assert!(!body.contains(">Follow<"), "no stale Follow state");

    // The graph row exists
    let jeff_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username = 'jeff'")
        .fetch_one(&app.db_pool)
        .await
        .unwrap();
    let alice_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username = 'alice'")
        .fetch_one(&app.db_pool)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM follows WHERE follower_id = $1 AND followee_id = $2",
    )
    .bind(alice_id)
    .bind(jeff_id)
    .fetch_one(&app.db_pool)
    .await
    .unwrap();
    assert_eq!(1, count, "graph row persisted");

    // Jeff's profile shows the follower count and alice's Following state
    let profile = app
        .api_client
        .get(format!("{}/u/jeff", app.address))
        .header("Cookie", &alice)
        .send()
        .await
        .unwrap();
    let body = profile.text().await.unwrap();
    assert!(body.contains("1 follower"), "follower count rendered");
    assert!(body.contains("Following"), "viewer sees own follow state");

    // Act — unfollow
    let response = app
        .api_client
        .delete(follow_url(&app.address, "jeff"))
        .header("Cookie", &alice)
        .header(DATASTAR, "true")
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — button flips back, row gone
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains(">Follow<"), "button flips back to Follow");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM follows WHERE follower_id = $1 AND followee_id = $2",
    )
    .bind(alice_id)
    .bind(jeff_id)
    .fetch_one(&app.db_pool)
    .await
    .unwrap();
    assert_eq!(0, count, "graph row removed");
}

#[tokio::test]
async fn duplicate_follow_is_idempotent() {
    // Arrange
    let app = spawn_app().await;
    let alice = register_and_login(&app, "alice", "alice@example.com", "wonderland1").await;
    let _jeff = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;

    // Act — follow twice
    for _ in 0..2 {
        let response = app
            .api_client
            .post(follow_url(&app.address, "jeff"))
            .header("Cookie", &alice)
            .header(DATASTAR, "true")
            .send()
            .await
            .unwrap();
        assert_eq!(200, response.status().as_u16());
    }

    // Assert — one row, one follower
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM follows WHERE follower_id = (SELECT id FROM users WHERE username = 'alice')
         AND followee_id = (SELECT id FROM users WHERE username = 'jeff')",
    )
    .fetch_one(&app.db_pool)
    .await
    .unwrap();
    assert_eq!(1, count, "duplicate follows collapse to one row");
}

#[tokio::test]
async fn follower_and_following_lists_render() {
    // Arrange — alice and bob follow jeff; jeff follows alice
    let app = spawn_app().await;
    let jeff = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    let alice = register_and_login(&app, "alice", "alice@example.com", "wonderland1").await;
    let bob = register_and_login(&app, "bob", "bob@example.com", "builder2").await;

    for follower in [&alice, &bob] {
        app.api_client
            .post(follow_url(&app.address, "jeff"))
            .header("Cookie", follower)
            .header(DATASTAR, "true")
            .send()
            .await
            .unwrap();
    }
    app.api_client
        .post(follow_url(&app.address, "alice"))
        .header("Cookie", &jeff)
        .header(DATASTAR, "true")
        .send()
        .await
        .unwrap();

    // Act — jeff's followers page
    let followers = app
        .api_client
        .get(format!("{}/u/jeff/followers", app.address))
        .send()
        .await
        .unwrap();

    // Assert — both followers listed
    assert_eq!(200, followers.status().as_u16());
    let body = followers.text().await.unwrap();
    assert!(body.contains("/u/alice"));
    assert!(body.contains("/u/bob"));

    // Act — jeff's following page
    let following = app
        .api_client
        .get(format!("{}/u/jeff/following", app.address))
        .send()
        .await
        .unwrap();

    // Assert — only alice (jeff follows alice, not bob)
    assert_eq!(200, following.status().as_u16());
    let body = following.text().await.unwrap();
    assert!(body.contains("/u/alice"));
    assert!(!body.contains("/u/bob"), "jeff does not follow bob");

    // Unknown account → styled 404
    let missing = app
        .api_client
        .get(format!("{}/u/nobody/followers", app.address))
        .send()
        .await
        .unwrap();
    assert_eq!(404, missing.status().as_u16());
}
