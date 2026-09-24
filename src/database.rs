// src/database.rs

use crate::authentication::{NewUser, User, UserStoreError};
use crate::follows::AccountSummary;
use crate::posts::{MediaRotation, PostCard, PostMediaView, PostPage, ProfileView};
use crate::services::media::{Derivative, StoredMedia};
use crate::utils::error_chain_fmt;
use actix_web::{ResponseError, http::StatusCode};
use async_trait::async_trait;
use thiserror::Error;
use uuid::Uuid;

pub mod postgres;

pub use postgres::*;

#[derive(Error)]
pub enum DatabaseError {
    #[error("record not found: {0}")]
    NotFound(String),

    #[error(transparent)]
    Operation(#[from] anyhow::Error),
}

impl std::fmt::Debug for DatabaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}

impl ResponseError for DatabaseError {
    fn status_code(&self) -> StatusCode {
        match self {
            DatabaseError::NotFound(_) => StatusCode::NOT_FOUND,
            DatabaseError::Operation(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Relational storage behind domain-shaped operations. Routes depend on
/// this trait, never on `PgPool` — the Postgres implementation lives in
/// `database::postgres`, and an alternative backend only has to satisfy
/// the same contract.
///
/// Readers that address a single record return `Option` (unknown id is a
/// normal outcome); `DatabaseError::NotFound` stays reserved for methods
/// where absence is exceptional.
#[async_trait]
pub trait DatabaseBackend: Send + Sync {
    /// Cheap connectivity probe for the deep health endpoint. The default
    /// suits backends with no remote failure mode; remote backends
    /// override it with a real round-trip.
    async fn ping(&self) -> Result<(), DatabaseError> {
        Ok(())
    }

    // ------------------------------------------------------------------
    // users
    // ------------------------------------------------------------------

    /// Insert a new live account. Duplicate handles/emails on live
    /// accounts map to the typed variants (the caller turns them into
    /// 409s); everything else is an operational error.
    async fn insert_user(&self, new_user: NewUser) -> Result<User, UserStoreError>;

    /// Find a live account by username or email (case-insensitive via
    /// CITEXT). Deleted accounts are invisible to the application.
    async fn find_user_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<User>, DatabaseError>;

    async fn find_user_id_by_username(&self, username: &str)
    -> Result<Option<Uuid>, DatabaseError>;

    // ------------------------------------------------------------------
    // social graph
    // ------------------------------------------------------------------

    /// Follow `followee` as `follower`. Idempotent — a duplicate is a
    /// no-op.
    async fn follow(&self, follower_id: Uuid, followee_id: Uuid) -> Result<(), DatabaseError>;

    /// Unfollow. Missing rows are a no-op (idempotent).
    async fn unfollow(&self, follower_id: Uuid, followee_id: Uuid) -> Result<(), DatabaseError>;

    async fn is_following(
        &self,
        follower_id: Uuid,
        followee_id: Uuid,
    ) -> Result<bool, DatabaseError>;

    async fn count_followers(&self, user_id: Uuid) -> Result<i64, DatabaseError>;

    async fn count_following(&self, user_id: Uuid) -> Result<i64, DatabaseError>;

    /// Accounts that follow `username`, newest follow first.
    async fn followers_of(&self, username: &str) -> Result<Vec<AccountSummary>, DatabaseError>;

    /// Accounts that `username` follows, newest follow first.
    async fn following_of(&self, username: &str) -> Result<Vec<AccountSummary>, DatabaseError>;

    // ------------------------------------------------------------------
    // posts + media
    // ------------------------------------------------------------------

    /// Persist a post and everything hanging off it — caption hashtags,
    /// media rows, derivative rows, post links, location — in a single
    /// transaction, so a half-uploaded post can never exist. The caller
    /// has already stored the media bytes; `media[i]` lands at
    /// `position = i`.
    async fn create_post_with_media(
        &self,
        user_id: Uuid,
        caption: &str,
        location_name: Option<String>,
        media: &[StoredMedia],
    ) -> Result<Uuid, DatabaseError>;

    /// Global chronological page (keyset-paginated). `before` is the
    /// last-seen post id; `None` = first page. The bool flags `has_more`.
    async fn load_recent_page(
        &self,
        before: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<PostCard>, bool), DatabaseError>;

    /// Page of posts carrying a specific hashtag.
    async fn load_tag_page(
        &self,
        tag: &str,
        before: Option<Uuid>,
        limit: i64,
    ) -> Result<(Vec<PostCard>, bool), DatabaseError>;

    /// Permalink data: the post, its owner, its media (with EXIF), its
    /// tags. `None` for unknown or deleted posts.
    async fn load_post_page(&self, post_id: Uuid) -> Result<Option<PostPage>, DatabaseError>;

    /// Media views for the post that owns `media_id` — used by the
    /// rotate fragment to re-render the block with bumped versions.
    async fn load_media_block_for_media(
        &self,
        media_id: Uuid,
    ) -> Result<Option<Vec<PostMediaView>>, DatabaseError>;

    /// The public view of an account: no email, no hash — only what a
    /// visitor may see. `None` for unknown accounts.
    async fn load_profile(
        &self,
        username: &str,
        viewer: Option<Uuid>,
    ) -> Result<Option<ProfileView>, DatabaseError>;

    /// The media row state the rotate flow needs, fetched in one go.
    async fn media_for_rotation(
        &self,
        media_id: Uuid,
    ) -> Result<Option<MediaRotation>, DatabaseError>;

    /// Persist a manual rotation: the cumulative angle, the rotated
    /// dimensions, a version bump (cache buster), and the regenerated
    /// derivatives' dimensions, sizes — and storage keys, so the database
    /// stays the single source of truth for where the bytes live.
    async fn apply_rotation(
        &self,
        media_id: Uuid,
        rotation: i32,
        width: u32,
        height: u32,
        derivatives: &[Derivative],
    ) -> Result<(), DatabaseError>;

    /// Storage key of a processed derivative variant (`thumb`, `medium`,
    /// `large`); `None` when the media or variant does not exist.
    async fn find_derivative_key(
        &self,
        media_id: Uuid,
        variant: &str,
    ) -> Result<Option<String>, DatabaseError>;
}
