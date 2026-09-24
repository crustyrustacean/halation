// tests/api/pages.rs

use crate::helpers::spawn_app;

#[tokio::test]
async fn home_page_renders() {
    // Arrange
    let app = spawn_app().await;

    // Act
    let response = app
        .api_client
        .get(format!("{}/", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert!(response.status().is_success());
    let body = response.text().await.expect("Expected an HTML body");
    assert!(
        body.contains("Halation"),
        "home page should identify the app"
    );
}

#[tokio::test]
async fn unknown_route_renders_the_404_page() {
    // Arrange
    let app = spawn_app().await;

    // Act
    let response = app
        .api_client
        .get(format!("{}/definitely-not-a-page", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(404, response.status().as_u16());
    let body = response.text().await.expect("Expected an HTML body");
    assert!(
        body.contains("exist"),
        "the 404 page should explain what happened"
    );
}
