// src/database/sqlite.rs

use crate::authentication::{NewUser, User, UserStoreError};
use crate::configuration::DatabaseSettings;
use crate::database::{DatabaseBackend, DatabaseError};
use crate::follows::AccountSummary;
use crate::posts::{
    CardMedia, PostCard, PostMediaView, PostPage, ProfileThumb, ProfileView, parse_hashtags,
};
use crate::services::media::StoredMedia;
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use uuid::Uuid;

/// The SQLite backend: every SQL statement in the application lives in this
/// file (plus the actix session store, which is infrastructure rather
/// than domain storage).
pub struct SqliteDatabase {
    pool: SqlitePool,
}

impl SqliteDatabase {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Ensure the parent directory exists, open the SQLite pool, and apply all
    /// embedded migrations.
    pub async fn connect(settings: &DatabaseSettings) -> Result<Self, anyhow::Error> {
        if let Some(path) = settings.file_path()
            && let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create parent directory {} for SQLite database",
                    parent.display()
                )
            })?;
        }

        let max_connections = if settings.is_in_memory() {
            1
        } else {
            settings.max_connections.unwrap_or(5)
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .connect_with(settings.connect_options())
            .await
            .with_context(|| format!("Failed to connect to SQLite at {:?}", settings.path))?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("Failed to run database migrations")?;
        tracing::info!(path = %settings.path, "database ready; migrations up to date");

        Ok(Self { pool })
    }

    pub fn pool(&self) -> SqlitePool {
        self.pool.clone()
    }
}

// Positional row types. sqlx decodes tuples happily, but bare tuples in
// annotations read terribly and trip `clippy::type_complexity` wherever
// the surrounding macro expansion doesn't hide them — so each shape gets
// a name, and the SELECT column order is documented once, right here.

/// `posts ⋈ users`: post id, owner id, caption, username, created_at, location.
type PostRow = (
    Uuid,
    Uuid,
    Option<String>,
    String,
    DateTime<Utc>,
    Option<String>,
);
/// `post_media ⋈ media` for a permalink: media id, EXIF JSON (as text), version.
type MediaExifRow = (Uuid, Option<String>, i32);
/// A live `users` row: id, username, display name, bio, created_at.
type ProfileRow = (Uuid, String, Option<String>, Option<String>, DateTime<Utc>);
/// Profile grid rows: post id, first media id (posts without media yield NULLs).
type ProfileThumbRow = (Uuid, Option<Uuid>, Option<i32>);
/// Feed page rows: post id, caption, location, username, created_at.
type FeedRow = (Uuid, Option<String>, Option<String>, String, DateTime<Utc>);
/// `post_media ⋈ media` for feeds: post id, media id, version.
type MediaVersionRow = (Uuid, Uuid, i32);
/// `post_hashtags ⋈ hashtags`: post id, tag.
type TagRow = (Uuid, String);

#[async_trait]
impl DatabaseBackend for SqliteDatabase {
    /// A bare `SELECT 1` proves the pool can query SQLite.
    async fn ping(&self) -> Result<(), DatabaseError> {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| DatabaseError::Operation(anyhow!("database ping failed: {e}")))?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // users
    // ------------------------------------------------------------------

    async fn insert_user(&self, new_user: NewUser) -> Result<User, UserStoreError> {
        let id = Uuid::new_v4();
        let now = Utc::now();

        sqlx::query_as::<_, User>(
            "INSERT INTO users (id, username, email, password_hash, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             RETURNING id, username, email, password_hash,
                       display_name, bio, created_at, updated_at, deleted_at",
        )
        .bind(id)
        .bind(&new_user.username)
        .bind(&new_user.email)
        .bind(new_user.password_hash.expose())
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if let Some(db) = e.as_database_error()
                && db.is_unique_violation()
            {
                let msg = db.message();
                if msg.contains("users.username") || msg.contains("users_username_live_idx") {
                    return UserStoreError::DuplicateUsername;
                }
                if msg.contains("users.email") || msg.contains("users_email_live_idx") {
                    return UserStoreError::DuplicateEmail;
                }
            }
            UserStoreError::Operation(anyhow!("Failed to insert user: {e}"))
        })
    }

    async fn find_user_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<User>, DatabaseError> {
        let user = sqlx::query_as::<_, User>(
            "SELECT id, username, email, password_hash,
                    display_name, bio, created_at, updated_at, deleted_at
             FROM users
             WHERE (username = ?1 OR email = ?1) AND deleted_at IS NULL
             LIMIT 1",
        )
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            DatabaseError::Operation(anyhow!("Failed to fetch user by identifier: {e}"))
        })?;

        Ok(user)
    }

    async fn find_user_id_by_username(
        &self,
        username: &str,
    ) -> Result<Option<Uuid>, DatabaseError> {
        sqlx::query_scalar("SELECT id FROM users WHERE username = ?1 AND deleted_at IS NULL")
            .bind(username)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| DatabaseError::Operation(anyhow!("Failed to find user: {e}")))
    }

    // ------------------------------------------------------------------
    // social graph
    // ------------------------------------------------------------------

    async fn follow(&self, follower_id: Uuid, followee_id: Uuid) -> Result<(), DatabaseError> {
        sqlx::query(
            "INSERT INTO follows (follower_id, followee_id, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT DO NOTHING",
        )
        .bind(follower_id)
        .bind(followee_id)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to follow: {e}")))?;
        Ok(())
    }

    async fn unfollow(&self, follower_id: Uuid, followee_id: Uuid) -> Result<(), DatabaseError> {
        sqlx::query("DELETE FROM follows WHERE follower_id = ?1 AND followee_id = ?2")
            .bind(follower_id)
            .bind(followee_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DatabaseError::Operation(anyhow!("Failed to unfollow: {e}")))?;
        Ok(())
    }

    async fn is_following(
        &self,
        follower_id: Uuid,
        followee_id: Uuid,
    ) -> Result<bool, DatabaseError> {
        let follows: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM follows WHERE follower_id = ?1 AND followee_id = ?2)",
        )
        .bind(follower_id)
        .bind(followee_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to check follow state: {e}")))?;
        Ok(follows)
    }

    async fn count_followers(&self, user_id: Uuid) -> Result<i64, DatabaseError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM follows WHERE followee_id = ?1")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| DatabaseError::Operation(anyhow!("Failed to count followers: {e}")))
    }

    async fn count_following(&self, user_id: Uuid) -> Result<i64, DatabaseError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM follows WHERE follower_id = ?1")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| DatabaseError::Operation(anyhow!("Failed to count following: {e}")))
    }

    async fn followers_of(&self, username: &str) -> Result<Vec<AccountSummary>, DatabaseError> {
        account_list(&self.pool, username, true).await
    }

    async fn following_of(&self, username: &str) -> Result<Vec<AccountSummary>, DatabaseError> {
        account_list(&self.pool, username, false).await
    }

    // ------------------------------------------------------------------
    // posts + media
    // ------------------------------------------------------------------

    async fn create_post_with_media(
        &self,
        user_id: Uuid,
        caption: &str,
        location_name: Option<String>,
        media: &[StoredMedia],
    ) -> Result<Uuid, DatabaseError> {
        let mut tx =
            self.pool.begin().await.map_err(|e| {
                DatabaseError::Operation(anyhow!("Failed to begin transaction: {e}"))
            })?;

        let post_id = Uuid::new_v4();
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO posts (id, user_id, caption, location_name, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(post_id)
        .bind(user_id)
        .bind(caption)
        .bind(location_name)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to insert post: {e}")))?;

        for tag in parse_hashtags(caption) {
            let candidate_id = Uuid::new_v4();
            let hashtag_id: Uuid = sqlx::query_scalar(
                "INSERT INTO hashtags (id, tag) VALUES (?1, ?2)
                 ON CONFLICT (tag) DO UPDATE SET tag = excluded.tag
                 RETURNING id",
            )
            .bind(candidate_id)
            .bind(&tag)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| DatabaseError::Operation(anyhow!("Failed to upsert hashtag: {e}")))?;

            sqlx::query(
                "INSERT INTO post_hashtags (post_id, hashtag_id) VALUES (?1, ?2)
                 ON CONFLICT DO NOTHING",
            )
            .bind(post_id)
            .bind(hashtag_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| DatabaseError::Operation(anyhow!("Failed to attach hashtag: {e}")))?;
        }

        for (position, stored) in media.iter().enumerate() {
            let exif_json = stored.exif.as_ref().map(|v| v.to_string());
            sqlx::query(
                "INSERT INTO media (id, owner_id, storage_key, mime_type, width, height, size_bytes, sha256, exif, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .bind(stored.media_id)
            .bind(user_id)
            .bind(&stored.storage_key)
            .bind(&stored.mime_type)
            .bind(stored.width as i32)
            .bind(stored.height as i32)
            .bind(stored.size_bytes)
            .bind(&stored.sha256)
            .bind(exif_json)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| DatabaseError::Operation(anyhow!("Failed to insert media: {e}")))?;

            for derivative in &stored.derivatives {
                sqlx::query(
                    "INSERT INTO media_derivatives (media_id, variant, storage_key, width, height, size_bytes)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .bind(stored.media_id)
                .bind(derivative.variant)
                .bind(&derivative.storage_key)
                .bind(derivative.width as i32)
                .bind(derivative.height as i32)
                .bind(derivative.size_bytes)
                .execute(&mut *tx)
                .await
                .map_err(|e| DatabaseError::Operation(anyhow!("Failed to insert derivative: {e}")))?;
            }

            sqlx::query("INSERT INTO post_media (post_id, media_id, position) VALUES (?1, ?2, ?3)")
                .bind(post_id)
                .bind(stored.media_id)
                .bind(position as i32)
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    DatabaseError::Operation(anyhow!("Failed to link media to post: {e}"))
                })?;
        }

        tx.commit()
            .await
            .map_err(|e| DatabaseError::Operation(anyhow!("Failed to commit post: {e}")))?;

        Ok(post_id)
    }

    async fn load_recent_page(
        &self,
        before: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<PostCard>, bool), DatabaseError> {
        let before = resolve_cursor(&self.pool, before).await?;
        load_page(&self.pool, before, None, limit).await
    }

    async fn load_tag_page(
        &self,
        tag: &str,
        before: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<PostCard>, bool), DatabaseError> {
        let before = resolve_cursor(&self.pool, before).await?;
        load_page(&self.pool, before, Some(&tag.to_lowercase()), limit).await
    }

    async fn load_post_page(&self, post_id: Uuid) -> Result<Option<PostPage>, DatabaseError> {
        let row: Option<PostRow> = sqlx::query_as(
            "SELECT p.id, p.user_id, p.caption, u.username, p.created_at, p.location_name
             FROM posts p JOIN users u ON u.id = p.user_id
             WHERE p.id = ?1",
        )
        .bind(post_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to load post: {e}")))?;

        let Some((id, owner_id, caption, username, created_at, location_name)) = row else {
            return Ok(None);
        };

        let media_rows: Vec<MediaExifRow> = sqlx::query_as(
            "SELECT pm.media_id, m.exif, m.version
             FROM post_media pm JOIN media m ON m.id = pm.media_id
             WHERE pm.post_id = ?1 ORDER BY pm.position",
        )
        .bind(post_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to load post media: {e}")))?;

        let media: Vec<PostMediaView> = media_rows
            .into_iter()
            .map(|(media_id, exif_json, version)| PostMediaView {
                media_id,
                exif: exif_json.and_then(|s| serde_json::from_str(&s).ok()),
                version,
            })
            .collect();

        let hashtags: Vec<String> = sqlx::query_scalar(
            "SELECT h.tag FROM post_hashtags ph
             JOIN hashtags h ON h.id = ph.hashtag_id
             WHERE ph.post_id = ?1 ORDER BY h.tag",
        )
        .bind(post_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to load post hashtags: {e}")))?;

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

    async fn load_profile(
        &self,
        username: &str,
        viewer: Option<Uuid>,
    ) -> Result<Option<ProfileView>, DatabaseError> {
        let row: Option<ProfileRow> = sqlx::query_as(
            "SELECT id, username, display_name, bio, created_at
             FROM users WHERE username = ?1 AND deleted_at IS NULL",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to load profile: {e}")))?;

        let Some((user_id, username, display_name, bio, created_at)) = row else {
            return Ok(None);
        };

        let post_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM posts WHERE user_id = ?1")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| DatabaseError::Operation(anyhow!("Failed to count posts: {e}")))?;

        // Rows are (post_id, Option<media_id>, Option<version>) — posts without
        // media are filtered out rather than failing the whole grid.
        let rows: Vec<ProfileThumbRow> = sqlx::query_as(
            "SELECT p.id, first_pm.media_id, first_pm.version
             FROM posts p
             LEFT JOIN (
                 SELECT pm.post_id, pm.media_id, m.version,
                        ROW_NUMBER() OVER (PARTITION BY pm.post_id ORDER BY pm.position) AS rn
                 FROM post_media pm
                 JOIN media m ON m.id = pm.media_id
             ) first_pm ON first_pm.post_id = p.id AND first_pm.rn = 1
             WHERE p.user_id = ?1
             ORDER BY p.created_at DESC, p.id DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to load profile posts: {e}")))?;

        let thumbs: Vec<ProfileThumb> = rows
            .into_iter()
            .filter_map(|(post_id, media_id, version)| {
                media_id.map(|media_id| ProfileThumb {
                    post_id,
                    media_id,
                    version: version.unwrap_or(1),
                })
            })
            .collect();

        let initial = username
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into());
        let follower_count = self.count_followers(user_id).await?;
        let following_count = self.count_following(user_id).await?;
        let is_following = match viewer {
            Some(viewer) if viewer != user_id => self.is_following(viewer, user_id).await?,
            _ => false,
        };
        let can_follow = viewer.is_some() && viewer != Some(user_id);

        Ok(Some(ProfileView {
            initial,
            display_name: display_name.unwrap_or_else(|| username.clone()),
            username,
            bio,
            joined_display: created_at.format("%B %Y").to_string(),
            post_count,
            follower_count,
            following_count,
            is_following,
            can_follow,
            thumbs,
        }))
    }

    async fn find_derivative_key(
        &self,
        media_id: Uuid,
        variant: &str,
    ) -> Result<Option<String>, DatabaseError> {
        sqlx::query_scalar(
            "SELECT storage_key FROM media_derivatives WHERE media_id = ?1 AND variant = ?2",
        )
        .bind(media_id)
        .bind(variant)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to find derivative: {e}")))
    }
}

// ---------------------------------------------------------------------------
// shared query helpers
// ---------------------------------------------------------------------------

/// Accounts joined through the social graph, newest follow first.
/// `followers = true` lists who follows `username`; otherwise the accounts
/// `username` follows.
async fn account_list(
    pool: &SqlitePool,
    username: &str,
    followers: bool,
) -> Result<Vec<AccountSummary>, DatabaseError> {
    const FOLLOWERS: &str = "SELECT u.username AS username,
         COALESCE(u.display_name, u.username) AS display_name,
         UPPER(SUBSTR(u.username, 1, 1)) AS initial
         FROM follows f
         JOIN users u ON u.id = f.follower_id
         JOIN users me ON me.id = f.followee_id
         WHERE me.username = ?1
         ORDER BY f.created_at DESC, u.username";

    const FOLLOWING: &str = "SELECT u.username AS username,
         COALESCE(u.display_name, u.username) AS display_name,
         UPPER(SUBSTR(u.username, 1, 1)) AS initial
         FROM follows f
         JOIN users u ON u.id = f.followee_id
         JOIN users me ON me.id = f.follower_id
         WHERE me.username = ?1
         ORDER BY f.created_at DESC, u.username";

    let rows: Vec<AccountSummary> = sqlx::query_as(if followers { FOLLOWERS } else { FOLLOWING })
        .bind(username)
        .fetch_all(pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to load account list: {e}")))?;

    // Mirror the SQL initial with the Rust-side computation so multibyte
    // first characters uppercase correctly.
    let rows = rows
        .into_iter()
        .map(|mut account| {
            account.initial = account
                .username
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default();
            account
        })
        .collect();

    Ok(rows)
}

/// Keyset-paginated post query. The cursor is the last-seen post id; its
/// `created_at` is resolved and used for a tuple comparison so posts
/// sharing a timestamp are never skipped or duplicated.
async fn load_page(
    pool: &SqlitePool,
    before: Option<(DateTime<Utc>, Uuid)>,
    tag: Option<&str>,
    limit: i64,
) -> Result<(Vec<PostCard>, bool), DatabaseError> {
    let cursor_created = before.as_ref().map(|(c, _)| *c);
    let cursor_id = before.as_ref().map(|(_, i)| *i);

    const FEED: &str = "SELECT p.id, p.caption, p.location_name, u.username, p.created_at
         FROM posts p
         JOIN users u ON u.id = p.user_id
         WHERE (?1 IS NULL OR p.created_at < ?1 OR (p.created_at = ?1 AND p.id < ?2))
         ORDER BY p.created_at DESC, p.id DESC
         LIMIT ?3";

    const TAGGED_FEED: &str =
        "SELECT p.id, p.caption, p.location_name, u.username, p.created_at
         FROM posts p
         JOIN users u ON u.id = p.user_id
         JOIN post_hashtags ph ON ph.post_id = p.id
         JOIN hashtags h ON h.id = ph.hashtag_id AND h.tag = ?4
         WHERE (?1 IS NULL OR p.created_at < ?1 OR (p.created_at = ?1 AND p.id < ?2))
         ORDER BY p.created_at DESC, p.id DESC
         LIMIT ?3";

    let mut query = sqlx::query_as::<_, FeedRow>(match tag {
        Some(_) => TAGGED_FEED,
        None => FEED,
    })
    .bind(cursor_created)
    .bind(cursor_id)
    .bind(limit + 1); // fetch one extra to detect has_more
    if let Some(tag) = tag {
        query = query.bind(tag.to_string());
    }

    let rows = query
        .fetch_all(pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to load feed page: {e}")))?;

    let has_more = rows.len() as i64 > limit;
    let rows = if has_more {
        &rows[..limit as usize]
    } else {
        &rows[..]
    };

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
    pool: &SqlitePool,
    post_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<CardMedia>>, DatabaseError> {
    if post_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT pm.post_id, m.id, m.version
         FROM post_media pm
         JOIN media m ON m.id = pm.media_id
         WHERE pm.post_id IN (",
    );
    let mut separated = qb.separated(", ");
    for id in post_ids {
        separated.push_bind(*id);
    }
    qb.push(") ORDER BY pm.post_id, pm.position");

    let rows: Vec<MediaVersionRow> = qb
        .build_query_as()
        .fetch_all(pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to load post media: {e}")))?;

    let mut map: std::collections::HashMap<Uuid, Vec<CardMedia>> = std::collections::HashMap::new();
    for (post_id, media_id, version) in rows {
        map.entry(post_id)
            .or_default()
            .push(CardMedia { media_id, version });
    }
    Ok(map)
}

async fn load_hashtags(
    pool: &SqlitePool,
    post_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<String>>, DatabaseError> {
    if post_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT ph.post_id, h.tag
         FROM post_hashtags ph
         JOIN hashtags h ON h.id = ph.hashtag_id
         WHERE ph.post_id IN (",
    );
    let mut separated = qb.separated(", ");
    for id in post_ids {
        separated.push_bind(*id);
    }
    qb.push(") ORDER BY h.tag");

    let rows: Vec<TagRow> = qb
        .build_query_as()
        .fetch_all(pool)
        .await
        .map_err(|e| DatabaseError::Operation(anyhow!("Failed to load post hashtags: {e}")))?;

    let mut map: std::collections::HashMap<Uuid, Vec<String>> = std::collections::HashMap::new();
    for (post_id, tag) in rows {
        map.entry(post_id).or_default().push(tag);
    }
    Ok(map)
}

/// Resolve a cursor post id to its (created_at, id) ordering key.
async fn resolve_cursor(
    pool: &SqlitePool,
    before: Option<Uuid>,
) -> Result<Option<(DateTime<Utc>, Uuid)>, DatabaseError> {
    let Some(before) = before else {
        return Ok(None);
    };
    let row: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT created_at FROM posts WHERE id = ?1")
            .bind(before)
            .fetch_optional(pool)
            .await
            .map_err(|e| DatabaseError::Operation(anyhow!("Failed to resolve cursor: {e}")))?;
    Ok(row.map(|created| (created, before)))
}
