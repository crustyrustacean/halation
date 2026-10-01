// tests/api/feed.rs

use crate::helpers::{register_and_login, spawn_app};
use image::DynamicImage;
use std::io::Cursor;
use uuid::Uuid;

/// Generate a real JPEG of the given dimensions.
fn test_jpeg(width: u32, height: u32) -> Vec<u8> {
    let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    }));
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .unwrap();
    buf
}

/// Upload one photo as the cookie's user; returns the permalink path.
async fn upload_photo(
    app: &crate::helpers::TestApp,
    cookie: &str,
    caption: &str,
    w: u32,
    h: u32,
) -> String {
    let part = reqwest::multipart::Part::bytes(test_jpeg(w, h))
        .file_name("photo.jpg")
        .mime_str("image/jpeg")
        .unwrap();
    let response = app
        .api_client
        .post(format!("{}/upload", app.address))
        .header("Cookie", cookie)
        .multipart(
            reqwest::multipart::Form::new()
                .text("caption", caption.to_string())
                .part("files", part),
        )
        .send()
        .await
        .expect("upload should succeed");
    assert_eq!(303, response.status().as_u16());
    response
        .headers()
        .get("Location")
        .and_then(|v| v.to_str().ok())
        .expect("upload should redirect")
        .to_string()
}

#[tokio::test]
async fn feed_page_shows_posts_newest_first() {
    // Arrange — two users, alternating posts
    let app = spawn_app().await;
    let jeff_cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    let alice_cookie = register_and_login(&app, "alice", "alice@example.com", "wonderland1").await;

    upload_photo(&app, &jeff_cookie, "first post by jeff", 600, 400).await;
    upload_photo(&app, &alice_cookie, "second post by alice", 600, 400).await;

    // Act — anonymous visitor sees the global timeline
    let response = app
        .api_client
        .get(format!("{}/", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — newest first
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.unwrap();
    let alice_pos = body
        .find("second post by alice")
        .expect("alice's post on feed");
    let jeff_pos = body
        .find("first post by jeff")
        .expect("jeff's post on feed");
    assert!(
        alice_pos < jeff_pos,
        "newest post (alice) must appear above older post (jeff)"
    );
    assert!(!body.contains("alice@example"), "no emails on public pages");
}

#[tokio::test]
async fn recent_page_shows_the_same_timeline() {
    // Arrange
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    upload_photo(&app, &cookie, "recent test", 600, 400).await;

    // Act
    let response = app
        .api_client
        .get(format!("{}/recent", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains("recent test"));
}

#[tokio::test]
async fn load_more_fragment_paginates_and_exhausts() {
    // Arrange — 12 posts, page size 10
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "paginator", "page@example.com", "hunter2hunter2").await;
    for i in 0..12 {
        upload_photo(&app, &cookie, &format!("caption number {i}"), 300, 200).await;
    }

    // Act — first fragment page
    let first = app
        .api_client
        .get(format!("{}/fragments/feed", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — SSE with two patches (cards + refreshed button)
    assert_eq!(200, first.status().as_u16());
    assert_eq!(
        "text/event-stream",
        first
            .headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok())
            .unwrap()
    );
    let body = first.text().await.unwrap();
    assert_eq!(
        2,
        body.matches("event: datastar-patch-elements").count(),
        "cards patch + button patch"
    );
    assert!(
        body.contains("mode before"),
        "cards insert before the button"
    );
    assert!(
        body.contains("mode replace"),
        "button is refreshed in place"
    );
    assert!(body.contains("caption number 11"), "newest card present");
    assert!(
        !body.contains("caption number 0"),
        "oldest card is on page 2"
    );

    // The next cursor is the 10th-newest post
    let tenth: Uuid = sqlx::query_scalar(
        "SELECT id FROM posts ORDER BY created_at DESC, id DESC OFFSET 9 LIMIT 1",
    )
    .fetch_one(&app.db_pool)
    .await
    .unwrap();

    // Act — second page with the cursor
    let second = app
        .api_client
        .get(format!("{}/fragments/feed?before={tenth}", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — remaining 2 cards, then the button is removed
    assert_eq!(200, second.status().as_u16());
    let body = second.text().await.unwrap();
    assert!(body.contains("caption number 0"));
    assert!(
        body.contains("mode remove"),
        "exhausted feed removes the button"
    );
}

#[tokio::test]
async fn location_chip_renders_on_the_feed() {
    // Arrange — upload, then set the location the geocoder would produce
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    upload_photo(&app, &cookie, "chip test", 600, 400).await;
    sqlx::query("UPDATE posts SET location_name = 'Discovery Park'")
        .execute(&app.db_pool)
        .await
        .unwrap();

    // Act
    let response = app
        .api_client
        .get(format!("{}/", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains("chip"), "location chip present");
    assert!(body.contains("Discovery Park"), "location name rendered");
}

#[tokio::test]
async fn hashtag_flow_from_caption_to_tag_page() {
    // Arrange
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    upload_photo(
        &app,
        &cookie,
        "The ten minutes when the windows catch fire. #goldenhour #kerrypark",
        600,
        400,
    )
    .await;

    // Act — feed renders tag links
    let feed = app
        .api_client
        .get(format!("{}/", app.address))
        .send()
        .await
        .expect("Failed to execute request.");
    let feed_body = feed.text().await.unwrap();

    // Assert — tags parsed, normalized, linked
    assert!(feed_body.contains("#goldenhour"));
    assert!(feed_body.contains("#kerrypark"));
    assert!(feed_body.contains("/hashtags/goldenhour"));

    // Act — the tag page carries the post
    let tag_page = app
        .api_client
        .get(format!("{}/hashtags/goldenhour", app.address))
        .send()
        .await
        .expect("Failed to execute request.");
    assert_eq!(200, tag_page.status().as_u16());
    let tag_body = tag_page.text().await.unwrap();
    assert!(tag_body.contains("windows catch fire"));

    // An unused tag page renders empty, not 404
    let empty = app
        .api_client
        .get(format!("{}/hashtags/nobodyusedthis", app.address))
        .send()
        .await
        .expect("Failed to execute request.");
    assert_eq!(200, empty.status().as_u16());
}

#[tokio::test]
async fn profile_page_renders_grid_and_counts() {
    // Arrange — two posts by jeff, one by alice
    let app = spawn_app().await;
    let jeff_cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    let alice_cookie = register_and_login(&app, "alice", "alice@example.com", "wonderland1").await;
    upload_photo(&app, &jeff_cookie, "jeff one", 600, 400).await;
    upload_photo(&app, &jeff_cookie, "jeff two", 600, 400).await;
    upload_photo(&app, &alice_cookie, "alice one", 600, 400).await;

    // Act
    let response = app
        .api_client
        .get(format!("{}/u/jeff", app.address))
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — jeff's grid shows only jeff's two posts
    assert_eq!(200, response.status().as_u16());
    let body = response.text().await.unwrap();
    assert!(body.contains("@jeff"));
    assert!(body.contains("2 posts"));
    assert_eq!(
        2,
        body.matches("/media/").count(),
        "two thumbnails in the grid"
    );
    assert!(
        !body.contains("alice one"),
        "alice's posts must not leak into jeff's grid"
    );

    // Unknown account → styled 404
    let missing = app
        .api_client
        .get(format!("{}/u/nobody", app.address))
        .send()
        .await
        .expect("Failed to execute request.");
    assert_eq!(404, missing.status().as_u16());
}

#[tokio::test]
async fn permalink_shows_the_photo_with_its_exif_row() {
    // Arrange — upload a JPEG carrying EXIF (fixture committed for this phase)
    let app = spawn_app().await;
    let cookie = register_and_login(&app, "jeff", "jeff@example.com", "hunter2hunter2").await;
    let exif_bytes = std::fs::read("tests/fixtures/exif_test.jpg").expect("EXIF fixture");
    let part = reqwest::multipart::Part::bytes(exif_bytes)
        .file_name("exif_test.jpg")
        .mime_str("image/jpeg");
    let response = app
        .api_client
        .post(format!("{}/upload", app.address))
        .header("Cookie", &cookie)
        .multipart(
            reqwest::multipart::Form::new()
                .text("caption", "with exif")
                .part("files", part.unwrap()),
        )
        .send()
        .await
        .expect("upload should succeed");
    let location = response
        .headers()
        .get("Location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();

    // Act
    let page = app
        .api_client
        .get(format!("{}{}", app.address, location))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("Failed to execute request.");

    // Assert — the photo page is the looking surface: the photograph at full
    // size, the caption, the owner, and the camera metadata. The EXIF row was
    // removed in 0.14.0 and restored — `media.exif` was never discarded, it
    // just stopped being rendered, which is exactly the sort of change a
    // test asserting absence would bless forever.
    assert_eq!(200, page.status().as_u16());
    let body = page.text().await.unwrap();
    assert!(body.contains("with exif"), "the caption renders");
    assert!(body.contains("jeff"), "the owner renders");
    assert!(body.contains("/media/"), "the photo renders");
    assert!(
        body.contains("/large"),
        "the photo page shows the large derivative"
    );
    assert!(body.contains("TestMake"), "the camera make renders");
    assert!(body.contains("TestModel"), "the camera model renders");
}
