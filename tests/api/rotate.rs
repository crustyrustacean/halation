// tests/api/rotate.rs

use crate::helpers::{register_and_login, spawn_app, upload_and_get_location};
use image::DynamicImage;
use uuid::Uuid;
use std::io::Cursor;

fn test_jpeg(width: u32, height: u32) -> Vec<u8> {
    let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    }));
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .unwrap();
    buf
}

async fn upload(app: &crate::helpers::TestApp, cookie: &str) -> Uuid {
    let location = crate::helpers::upload_and_get_location(app, cookie, "rotate me", 600, 400).await;
    let post_id = location.trim_start_matches("/p/");
    sqlx::query_scalar("SELECT media_id FROM post_media WHERE post_id = $1")
        .bind(Uuid::parse_str(post_id).unwrap())
        .fetch_one(&app.db_pool)
        .await
        .expect("media row should exist")
}

#[tokio::test]
async fn owner_can_rotate_and_version_bumps() {
    // Arrange — jeff uploads a landscape photo
    let app = spawn_app().await;
    let jeff = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    let alice = register_and_login(&app, "alice", "alice@example.com", "wonderland1").await;
    let media_id = upload(&app, &jeff).await;

    let (width, version): (i32, i32) =
        sqlx::query_as("SELECT width, version FROM media WHERE id = $1")
            .bind(media_id)
            .fetch_one(&app.db_pool)
            .await
            .unwrap();
    assert_eq!((600, 1), (width, version));

    // Act — alice (not the owner) tries to rotate: forbidden
    let forbidden = app
        .api_client
        .post(&format!(
            "{}/fragments/media/{media_id}/rotate",
            &app.address
        ))
        .header("Cookie", &alice)
        .header("Datastar-Request", "true")
        .send()
        .await
        .expect("Failed to execute request.");
    assert_eq!(403, forbidden.status().as_u16());

    // Act — the owner rotates 90° clockwise
    let rotated = app
        .api_client
        .post(&format!(
            "{}/fragments/media/{media_id}/rotate",
            &app.address
        ))
        .header("Cookie", &jeff)
        .header("Datastar-Request", "true")
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — SSE response, dimensions swapped, version bumped
    assert_eq!(200, rotated.status().as_u16());
    let (width, height, version): (i32, i32, i32) =
        sqlx::query_as("SELECT width, height, version FROM media WHERE id = $1")
            .bind(media_id)
            .fetch_one(&app.db_pool)
            .await
            .unwrap();
    assert_eq!((400, 600), (width, height), "landscape becomes portrait");
    assert_eq!(2, version, "cache-buster bumped");

    // The medium derivative was regenerated to match
    let (w, h): (i32, i32) = sqlx::query_as(
        "SELECT width, height FROM media_derivatives WHERE media_id = $1 AND variant = 'medium'",
    )
    .bind(media_id)
    .fetch_one(&app.db_pool)
    .await
    .unwrap();
    assert_eq!((400, 600), (w, h), "medium derives from the rotated image");
}

#[tokio::test]
async fn unauthenticated_rotate_is_unauthorized() {
    // Arrange
    let app = spawn_app().await;
    let jeff = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    let media_id = upload(&app, &jeff).await;

    // Act — no cookie at all
    let response = app
        .api_client
        .post(&format!(
            "{}/fragments/media/{media_id}/rotate",
            &app.address
        ))
        .header("Datastar-Request", "true")
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(401, response.status().as_u16());
}
