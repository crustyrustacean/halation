// tests/api/upload.rs

use crate::helpers::{register_and_login, spawn_app};
use image::DynamicImage;
use std::io::Cursor;

/// Generate a real JPEG of the given dimensions for pipeline round-trips.
fn test_jpeg(width: u32, height: u32) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    }));
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .unwrap();
    buf
}

#[tokio::test]
async fn upload_page_renders_when_logged_in() {
    // Arrange — covers the logged-in nav branch AND the page's context
    // completeness (the errors list is absent on a fresh GET)
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;

    // Act
    let response = app
        .api_client
        .get(&format!("{}/upload", &app.address))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains("Photos"), "the composer form renders");
    assert!(body.contains("Log out"), "nav shows the logged-in branch");
}

#[tokio::test]
async fn permalink_without_caption_renders() {
    // Arrange — a post with a NULL caption, inserted directly
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    let user_id: Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username = 'jeff'")
        .fetch_one(&app.db_pool)
        .await
        .unwrap();
    let post_id: Uuid = sqlx::query_scalar(
        "INSERT INTO posts (user_id, caption) VALUES ($1, NULL) RETURNING id",
    )
    .bind(user_id)
    .fetch_one(&app.db_pool)
    .await
    .unwrap();

    // Act
    let response = app
        .api_client
        .get(&format!("{}/p/{post_id}", &app.address))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — a caption-less post renders without a template error
    assert_eq!(200, response.status().as_u16());
}

#[tokio::test]
async fn upload_requires_login() {
    // Arrange
    let app = spawn_app().await;
    let part = reqwest::multipart::Part::bytes(test_jpeg(100, 100))
        .file_name("test.jpg")
        .mime_str("image/jpeg")
        .unwrap();

    // Act — anonymous page visit
    let page = app
        .api_client
        .get(&format!("{}/upload", &app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Act — anonymous multipart submit
    let submit = app
        .api_client
        .post(&format!("{}/upload", &app.address))
        .multipart(
            reqwest::multipart::Form::new()
                .text("caption", "sneaky")
                .part("files", part),
        )
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — both gated to login
    assert_eq!(303, page.status().as_u16());
    assert_eq!(
        Some("/login"),
        page.headers().get("Location").and_then(|v| v.to_str().ok())
    );
    assert_eq!(303, submit.status().as_u16());
    assert_eq!(
        Some("/login"),
        submit.headers().get("Location").and_then(|v| v.to_str().ok())
    );
}

#[tokio::test]
async fn upload_round_trip_creates_post_and_serves_derivatives() {
    // Arrange — a logged-in user and a 1600x1000 JPEG
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    let part = reqwest::multipart::Part::bytes(test_jpeg(1600, 1000))
        .file_name("golden_hour.jpg")
        .mime_str("image/jpeg")
        .unwrap();

    // Act — upload
    let response = app
        .api_client
        .post(&format!("{}/upload", &app.address))
        .header("Cookie", &cookie)
        .multipart(
            reqwest::multipart::Form::new()
                .text("caption", "Olympics doing that thing again tonight.")
                .part("files", part),
        )
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — redirect to the permalink
    assert_eq!(303, response.status().as_u16());
    let location = response
        .headers()
        .get("Location")
        .and_then(|v| v.to_str().ok())
        .expect("upload should redirect")
        .to_string();
    assert!(location.starts_with("/p/"), "redirect target is the permalink");

    let post_id = location.trim_start_matches("/p/");

    // Media + derivatives persisted, owned by the uploader
    let media_row: (Uuid, i64) = sqlx::query_as(
        "SELECT m.id, m.size_bytes FROM media m
         JOIN posts p ON p.id = (SELECT post_id FROM post_media LIMIT 1)
         JOIN post_media pm ON pm.media_id = m.id
         WHERE m.owner_id = (SELECT id FROM users WHERE username = 'jeff')
         LIMIT 1",
    )
    .fetch_one(&app.db_pool)
    .await
    .expect("media row should exist and be owned");
    assert!(media_row.1 > 0);

    let variants: Vec<String> =
        sqlx::query_scalar("SELECT variant FROM media_derivatives WHERE media_id = $1")
            .bind(media_row.0)
            .fetch_all(&app.db_pool)
            .await
            .unwrap();
    assert_eq!(3, variants.len());

    // Permalink renders the photo (medium variant) with the caption
    let page = app
        .api_client
        .get(&format!("{}{}", &app.address, location))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("Failed to execute request.");
    assert_eq!(200, page.status().as_u16());
    let body = page.text().await.unwrap();
    assert!(body.contains("Olympics doing that thing again tonight."));
    assert!(body.contains(&format!("/media/{}/medium", media_row.0)));
    assert!(body.contains("jeff"));

    // Serving: medium variant is a real 640x400 JPEG with immutable cache
    let served = app
        .api_client
        .get(&format!(
            "{}/media/{}/medium",
            &app.address, media_row.0
        ))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("Failed to execute request.");
    assert_eq!(200, served.status().as_u16());
    assert_eq!(
        "image/jpeg",
        served.headers().get("Content-Type").and_then(|v| v.to_str().ok()).unwrap()
    );
    assert_eq!(
        "public, max-age=31536000, immutable",
        served.headers().get("Cache-Control").and_then(|v| v.to_str().ok()).unwrap()
    );
    let bytes = served.bytes().await.unwrap();
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!((640, 400), (decoded.width(), decoded.height()));

    // Privacy: the served derivative carries no EXIF
    assert!(
        !bytes.windows(6).any(|window| window == b"Exif\0\0"),
        "derivatives must be EXIF-free"
    );
}

#[tokio::test]
async fn upload_rejects_non_image_with_422() {
    // Arrange
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    let part = reqwest::multipart::Part::bytes(b"this is not an image".to_vec())
        .file_name("notes.txt")
        .mime_str("text/plain")
        .unwrap();

    // Act
    let response = app
        .api_client
        .post(&format!("{}/upload", &app.address))
        .header("Cookie", &cookie)
        .multipart(
            reqwest::multipart::Form::new()
                .text("caption", "oops")
                .part("files", part),
        )
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(422, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains("supported image"));
}

#[tokio::test]
async fn original_variant_is_never_publicly_served() {
    // Arrange — upload one photo
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    let response = app
        .api_client
        .post(&format!("{}/upload", &app.address))
        .header("Cookie", &cookie)
        .multipart(
            reqwest::multipart::Form::new()
                .text("caption", "private original")
                .part(
                    "files",
                    reqwest::multipart::Part::bytes(test_jpeg(300, 300))
                        .file_name("x.jpg")
                        .mime_str("image/jpeg")
                        .unwrap(),
                ),
        )
        .send()
        .await
        .expect("upload should succeed");
    let location = response.headers().get("Location").and_then(|v| v.to_str().ok()).unwrap().to_string();
    let post_id = location.trim_start_matches("/p/");
    let media_id: Uuid =
        sqlx::query_scalar("SELECT media_id FROM post_media WHERE post_id = $1")
            .bind(Uuid::parse_str(post_id).unwrap())
            .fetch_one(&app.db_pool)
            .await
            .unwrap();

    // Act — request the original (EXIF-bearing) bytes
    let response = app
        .api_client
        .get(&format!("{}/media/{}/original", &app.address, media_id))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — originals are owner-private, period
    assert_eq!(404, response.status().as_u16());
}

#[tokio::test]
async fn photo_set_upload_stores_multiple_images_in_order() {
    // Arrange — a 2-up photo set, mockup-style
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;

    let form = reqwest::multipart::Form::new()
        .text("caption", "Fort Worden, last Sunday.")
        .part(
            "files",
            reqwest::multipart::Part::bytes(test_jpeg(900, 600))
                .file_name("a.jpg")
                .mime_str("image/jpeg")
                .unwrap(),
        )
        .part(
            "files",
            reqwest::multipart::Part::bytes(test_jpeg(900, 600))
                .file_name("b.jpg")
                .mime_str("image/jpeg")
                .unwrap(),
        );

    // Act
    let response = app
        .api_client
        .post(&format!("{}/upload", &app.address))
        .header("Cookie", &cookie)
        .multipart(form)
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — both images attached, positions preserved
    assert_eq!(303, response.status().as_u16());
    let rows: Vec<(Uuid, i32)> = sqlx::query_as(
        "SELECT media_id, position FROM post_media ORDER BY position",
    )
    .fetch_all(&app.db_pool)
    .await
    .unwrap();
    assert_eq!(2, rows.len());
    assert_eq!(vec![0, 1], rows.iter().map(|r| r.1).collect::<Vec<_>>());
    let _ = DynamicImage::new_rgb8(1, 1); // keep image crate import honest
}

use uuid::Uuid;
