// src/posts.rs
//!
//! Post and profile view types plus caption parsing — the vocabulary the
//! routes and templates share. Every SQL statement that fills these types
//! lives in `database::postgres`, behind the `DatabaseBackend` trait.

use serde::Serialize;
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
// view types
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
    pub follower_count: i64,
    pub following_count: i64,
    /// Viewer-relative: true only when a logged-in viewer follows this account.
    pub is_following: bool,
    /// Viewer-relative: whether the follow/unfollow button should render.
    pub can_follow: bool,
    pub thumbs: Vec<ProfileThumb>,
}

#[derive(Debug, Serialize)]
pub struct ProfileThumb {
    pub post_id: Uuid,
    pub media_id: Uuid,
    pub version: i32,
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
