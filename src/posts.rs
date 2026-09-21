// src/posts.rs
//!
//! Post reading and writing: feed loading with keyset (cursor) pagination,
//! hashtag parsing, and the permalink/profile data loaders.

use anyhow::anyhow;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// How many post cards a single page (feed, recent, hashtag) carries.
pub const FEED_PAGE_SIZE: i64 = 10;

/// A rendered post card. `media` holds media ids (templates build
/// `/media/{id}/{variant}` URLs); `hashtags` is normalized lowercase.
#[derive(Debug, Clone, Serialize)]
pub struct CardMedia {
    pub media_id: Uuid,
    pub version: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct PostCard {
    pub id: Uuid,
    pub caption: Option<String>,
    pub location_name: Option<String>,
    pub username: String,
    pub created_at_display: String,
    pub media: Vec<CardMedia>,
    pub hashtags: Vec<String>,
}

// ---------------------------------------------------------------------------
// hashtag parsing
// ---------------------------------------------------------------------------

/// Extract `#tag` tokens from a caption. Tags are lowercase
/// `[a-z0-9_]+`, deduplicated, capped at 50 characters.
pub fn parse_hashtags(caption: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    let mut chars = caption.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == '#' {
            chars.next();
            let mut tag = String::new();
            while let Some(&t) = chars.peek() {
                if t.is_ascii_alphanumeric() || t == '_' {
                    tag.push(t.to_ascii_lowercase());
                    chars.next();
                } else {
                    break;
                }
            }
            if !tag.is_empty() && tag.len() <= 50 && !tags.contains(&tag) {
                tags.push(tag);
            }
        } else {
            chars.next();
        }
    }
    tags
}

// ---------------------------------------------------------------------------
// writing
// ---------------------------------------------------------------------------

/// Insert a post and attach its caption's hashtags. Runs inside the
/// caller's transaction so a post and its tags commit atomically.
pub async fn create_post(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
    caption: &str,
) -> Result<Uuid, anyhow::Error> {
    let post_id: Uuid =
        sqlx::query_scalar("INSERT INTO posts (user_id, caption) VALUES ($1, $2) RETURNING id")
            .bind(user_id)
            .bind(caption)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| anyhow!("Failed to insert post: {e}"))?;

    for tag in parse_hashtags(caption) {
        let hashtag_id: Uuid = sqlx::query_scalar(
            "INSERT INTO hashtags (tag) VALUES ($1)
             ON CONFLICT (tag) DO UPDATE SET tag = EXCLUDED.tag
             RETURNING id",
        )
        .bind(&tag)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| anyhow!("Failed to upsert hashtag: {e}"))?;

        sqlx::query(
            "INSERT INTO post_hashtags (post_id, hashtag_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(post_id)
        .bind(hashtag_id)
        .execute(&mut *conn)
        .await
        .map_err(|e| anyhow!("Failed to attach hashtag: {e}"))?;
    }

    Ok(post_id)
}

// ---------------------------------------------------------------------------
// reading — feed pages
// ---------------------------------------------------------------------------

/// Keyset-paginated post query. The cursor is the last-seen post id; its
/// `created_at` is resolved and used for a row-value comparison so posts
/// sharing a timestamp are never skipped or duplicated.
async fn load_page(
    pool: &PgPool,
    before: Option<(DateTime<Utc>, Uuid)>,
    tag: Option<&str>,
    limit: i64,
) -> Result<(Vec<PostCard>, bool), anyhow::Error> {
    let cursor_created = before.as_ref().map(|(c, _)| *c);
    let cursor_id = before.as_ref().map(|(_, i)| *i);

    let tag_filter = if tag.is_some() {
        "JOIN post_hashtags ph ON ph.post_id = p.id
         JOIN hashtags h ON h.id = ph.hashtag_id AND h.tag = $4"
    } else {
        ""
    };

    let sql = format!(
        "SELECT p.id, p.caption, p.location_name, u.username::text, p.created_at
         FROM posts p
         JOIN users u ON u.id = p.user_id
         {tag_filter}
         WHERE (p.created_at, p.id) < (
             COALESCE($1, 'infinity'::timestamptz),
             COALESCE($2, '00000000-0000-0000-0000-000000000000'::uuid)
         )
         ORDER BY p.created_at DESC, p.id DESC
         LIMIT $3"
    );

    let mut query =
        sqlx::query_as::<_, (Uuid, Option<String>, Option<String>, String, DateTime<Utc>)>(&sql)
        .bind(cursor_created)
        .bind(cursor_id)
        .bind(limit + 1); // fetch one extra to detect has_more
    if let Some(tag) = tag {
        query = query.bind(tag.to_string());
    }

    let rows = query
        .fetch_all(pool)
        .await
        .map_err(|e| anyhow!("Failed to load feed page: {e}"))?;

    let has_more = rows.len() as i64 > limit;
    let rows = if has_more { &rows[..limit as usize] } else { &rows[..] };

    let ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    let media_map = load_media_ids(pool, &ids).await?;
    let tag_map = load_hashtags(pool, &ids).await?;

    let cards = rows
        .iter()
        .map(|r| PostCard {
            id: r.0,
            caption: r.1.clone(),
            location_name: r.2.clone(),
            username: r.3.clone(),
            created_at_display: r.4.format("%b %d").to_string(),
            media: media_map.get(&r.0).cloned().unwrap_or_default(),
            hashtags: tag_map.get(&r.0).cloned().unwrap_or_default(),
        })
        .collect();

    Ok((cards, has_more))
}

async fn load_media_ids(
    pool: &PgPool,
    post_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<CardMedia>>, anyhow::Error> {
    if post_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows: Vec<(Uuid, Uuid, i32)> = sqlx::query_as(
        "SELECT pm.post_id, m.id, m.version
         FROM post_media pm
         JOIN media m ON m.id = pm.media_id
         WHERE pm.post_id = ANY($1)
         ORDER BY pm.post_id, pm.position",
    )
    .bind(post_ids)
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow!("Failed to load post media: {e}"))?;

    let mut map: std::collections::HashMap<Uuid, Vec<CardMedia>> = std::collections::HashMap::new();
    for (post_id, media_id, version) in rows {
        map.entry(post_id).or_default().push(CardMedia { media_id, version });
    }
    Ok(map)
}

async fn load_hashtags(
    pool: &PgPool,
    post_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<String>>, anyhow::Error> {
    if post_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT ph.post_id, h.tag::text
         FROM post_hashtags ph
         JOIN hashtags h ON h.id = ph.hashtag_id
         WHERE ph.post_id = ANY($1)
         ORDER BY h.tag",
    )
    .bind(post_ids)
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow!("Failed to load post hashtags: {e}"))?;

    let mut map: std::collections::HashMap<Uuid, Vec<String>> = std::collections::HashMap::new();
    for (post_id, tag) in rows {
        map.entry(post_id).or_default().push(tag);
    }
    Ok(map)
}

/// Global chronological page. The cursor is the last-seen post id
/// (None = first page).
pub async fn load_recent_page(
    pool: &PgPool,
    before: Option<Uuid>,
    limit: i64,
) -> Result<(Vec<PostCard>, bool), anyhow::Error> {
    let before = resolve_cursor(pool, before).await?;
    load_page(pool, before, None, limit).await
}

/// Page of posts carrying a specific hashtag.
pub async fn load_tag_page(
    pool: &PgPool,
    tag: &str,
    before: Option<Uuid>,
    limit: i64,
) -> Result<(Vec<PostCard>, bool), anyhow::Error> {
    let before = resolve_cursor(pool, before).await?;
    load_page(pool, before, Some(&tag.to_lowercase()), limit).await
}

/// Resolve a cursor post id to its (created_at, id) ordering key.
async fn resolve_cursor(
    pool: &PgPool,
    before: Option<Uuid>,
) -> Result<Option<(DateTime<Utc>, Uuid)>, anyhow::Error> {
    let Some(before) = before else {
        return Ok(None);
    };
    let row: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT created_at FROM posts WHERE id = $1")
            .bind(before)
            .fetch_optional(pool)
            .await
            .map_err(|e| anyhow!("Failed to resolve cursor: {e}"))?;
    Ok(row.map(|created| (created, before)))
}

// ---------------------------------------------------------------------------
// reading — permalink & profile
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PostMediaView {
    pub media_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exif: Option<serde_json::Value>,
    pub version: i32,
}

#[derive(Debug, Serialize)]
pub struct PostPage {
    pub post_id: Uuid,
    pub owner_id: Uuid,
    pub caption: Option<String>,
    pub username: String,
    pub created_at_display: String,
    pub location_name: Option<String>,
    pub media: Vec<PostMediaView>,
    pub hashtags: Vec<String>,
}

/// Permalink data: the post, its owner, its media (with EXIF), its tags.
/// Returns `None` for unknown or deleted posts.
pub async fn load_post_page(pool: &PgPool, post_id: Uuid) -> Result<Option<PostPage>, anyhow::Error> {
    let row: Option<(Uuid, Uuid, Option<String>, String, DateTime<Utc>, Option<String>)> =
        sqlx::query_as(
            "SELECT p.id, p.user_id, p.caption, u.username::text, p.created_at, p.location_name
             FROM posts p JOIN users u ON u.id = p.user_id
             WHERE p.id = $1",
        )
        .bind(post_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| anyhow!("Failed to load post: {e}"))?;

    let Some((id, owner_id, caption, username, created_at, location_name)) = row else {
        return Ok(None);
    };

    // JSONB decoded via ::text + serde (consistent with the sessions table).
    let media_rows: Vec<(Uuid, Option<String>, i32)> = sqlx::query_as(
        "SELECT pm.media_id, m.exif::text, m.version
         FROM post_media pm JOIN media m ON m.id = pm.media_id
         WHERE pm.post_id = $1 ORDER BY pm.position",
    )
    .bind(post_id)
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow!("Failed to load post media: {e}"))?;

    let media: Vec<PostMediaView> = media_rows
        .into_iter()
        .map(|(media_id, exif_json, version)| PostMediaView {
            media_id,
            exif: exif_json.and_then(|s| serde_json::from_str(&s).ok()),
            version,
        })
        .collect();

    let hashtags: Vec<String> = sqlx::query_scalar(
        "SELECT h.tag::text FROM post_hashtags ph
         JOIN hashtags h ON h.id = ph.hashtag_id
         WHERE ph.post_id = $1 ORDER BY h.tag",
    )
    .bind(post_id)
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow!("Failed to load post hashtags: {e}"))?;

    Ok(Some(PostPage {
        post_id: id,
        owner_id,
        caption,
        username,
        created_at_display: created_at.format("%b %d").to_string(),
        location_name,
        media,
        hashtags,
    }))
}

/// Media views for the post that owns `media_id` — used by the rotate
/// fragment to re-render the block with bumped versions.
pub async fn load_post_media_block_for_media(
    pool: &PgPool,
    media_id: Uuid,
) -> Result<Option<Vec<PostMediaView>>, anyhow::Error> {
    let post_id: Option<Uuid> =
        sqlx::query_scalar("SELECT post_id FROM post_media WHERE media_id = $1 LIMIT 1")
            .bind(media_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| anyhow!("Failed to find post for media: {e}"))?;

    let Some(post_id) = post_id else {
        return Ok(None);
    };

    let media: Vec<PostMediaView> = sqlx::query_as(
        "SELECT pm.media_id, m.exif::text, m.version
         FROM post_media pm JOIN media m ON m.id = pm.media_id
         WHERE pm.post_id = $1 ORDER BY pm.position",
    )
    .bind(post_id)
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow!("Failed to load media block: {e}"))?;

    Ok(Some(media))
}

/// The public view of an account: no email, no hash — only what a
/// visitor may see.
#[derive(Debug, Serialize)]
pub struct ProfileView {
    pub username: String,
    pub initial: String,
    pub display_name: String,
    pub bio: Option<String>,
    pub joined_display: String,
    pub post_count: i64,
    pub thumbs: Vec<ProfileThumb>,
}

#[derive(Debug, Serialize)]
pub struct ProfileThumb {
    pub post_id: Uuid,
    pub media_id: Uuid,
    pub version: i32,
}

pub async fn load_profile(
    pool: &PgPool,
    username: &str,
) -> Result<Option<ProfileView>, anyhow::Error> {
    let row: Option<(Uuid, String, Option<String>, Option<String>, DateTime<Utc>)> =
        sqlx::query_as(
            "SELECT id, username::text, display_name, bio, created_at
             FROM users WHERE username = $1 AND deleted_at IS NULL",
        )
        .bind(username)
        .fetch_optional(pool)
        .await
        .map_err(|e| anyhow!("Failed to load profile: {e}"))?;

    let Some((user_id, username, display_name, bio, created_at)) = row else {
        return Ok(None);
    };

    let post_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM posts WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(pool)
            .await
            .map_err(|e| anyhow!("Failed to count posts: {e}"))?;

    // Rows are (post_id, Option<media_id>, Option<version>) — posts without
    // media are filtered out rather than failing the whole grid.
    let rows: Vec<(Uuid, Option<Uuid>, Option<i32>)> = sqlx::query_as(
        "SELECT p.id, first_pm.media_id, first_pm.version
         FROM posts p
         LEFT JOIN LATERAL (
             SELECT pm.media_id, m.version
             FROM post_media pm JOIN media m ON m.id = pm.media_id
             WHERE pm.post_id = p.id ORDER BY pm.position LIMIT 1
         ) first_pm ON true
         WHERE p.user_id = $1
         ORDER BY p.created_at DESC, p.id DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow!("Failed to load profile posts: {e}"))?;
    let thumbs: Vec<ProfileThumb> = rows
        .into_iter()
        .filter_map(|(post_id, media_id, version)| {
            media_id.map(|media_id| ProfileThumb { post_id, media_id, version: version.unwrap_or(1) })
        })
        .collect();

    let initial = username
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    Ok(Some(ProfileView {
        initial,
        display_name: display_name.unwrap_or_else(|| username.clone()),
        username,
        bio,
        joined_display: created_at.format("%B %Y").to_string(),
        post_count,
        thumbs,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mixed_case_and_punctuation() {
        // Act
        let tags = parse_hashtags("Sunset over #PugetSound and #golden_hour! #PugetSound again.");

        // Assert — lowercase, deduplicated, punctuation stripped
        assert_eq!(tags, vec!["pugetsound", "golden_hour"]);
    }

    #[test]
    fn bare_hash_and_overlong_tag_are_ignored() {
        // Act
        let tags = parse_hashtags("# # lone# and #four Score, #repeated #repeated");

        // Assert — "# " with no tag yields nothing; lone# is not a tag;
        // four passes; repeated dedupes
        assert_eq!(tags, vec!["four", "repeated"]);
    }

    #[test]
    fn caption_without_hashtags_yields_empty() {
        assert!(parse_hashtags("no tags here at all").is_empty());
        assert!(parse_hashtags("").is_empty());
    }
}
