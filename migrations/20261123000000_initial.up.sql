-- Initial SQLite schema for Halation.
-- UUIDs are stored as BLOB (sqlx native uuid encoding) and generated in Rust.
-- Timestamps are RFC3339 UTC strings (TEXT) so lexicographical order matches
-- chronological order. Case-insensitive columns use COLLATE NOCASE.

CREATE TABLE users (
    id              BLOB PRIMARY KEY NOT NULL,
    username        TEXT NOT NULL COLLATE NOCASE,
    email           TEXT NOT NULL COLLATE NOCASE,
    password_hash   TEXT NOT NULL,
    display_name    TEXT,
    bio             TEXT,
    avatar_media_id BLOB REFERENCES media(id),
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    deleted_at      TEXT
);

-- Uniqueness among live accounts only; handles/emails recycle after deletion.
CREATE UNIQUE INDEX users_username_live_idx ON users (username) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX users_email_live_idx    ON users (email)    WHERE deleted_at IS NULL;

-- id is actix-session's session key: a 64-char alphanumeric token, hence TEXT.
CREATE TABLE sessions (
    id           TEXT PRIMARY KEY NOT NULL,
    user_id      BLOB REFERENCES users(id) ON DELETE CASCADE,
    state        TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    expires_at   TEXT,
    last_seen_at TEXT,
    user_agent   TEXT,
    ip           TEXT
);

CREATE INDEX sessions_user_id_idx    ON sessions (user_id);
CREATE INDEX sessions_expires_at_idx ON sessions (expires_at);

CREATE TABLE media (
    id          BLOB PRIMARY KEY NOT NULL,
    owner_id    BLOB NOT NULL REFERENCES users(id),
    kind        TEXT NOT NULL DEFAULT 'photo',
    storage_key TEXT NOT NULL,
    mime_type   TEXT NOT NULL,
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    size_bytes  INTEGER NOT NULL,
    sha256      TEXT NOT NULL,
    -- Display-only EXIF JSON copied out of the original; served derivatives are stripped.
    exif        TEXT,
    version     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);

CREATE INDEX media_owner_id_idx ON media (owner_id);

CREATE TABLE media_derivatives (
    media_id    BLOB NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    variant     TEXT NOT NULL,
    storage_key TEXT NOT NULL,
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    size_bytes  INTEGER NOT NULL,
    PRIMARY KEY (media_id, variant)
);

CREATE TABLE posts (
    id            BLOB PRIMARY KEY NOT NULL,
    user_id       BLOB NOT NULL REFERENCES users(id),
    caption       TEXT,
    location_name TEXT,
    lat           REAL,
    lng           REAL,
    filter        TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'))
);

CREATE INDEX posts_user_id_created_at_idx ON posts (user_id, created_at DESC);
CREATE INDEX posts_created_at_idx         ON posts (created_at DESC);

CREATE TABLE post_media (
    post_id  BLOB NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    media_id BLOB NOT NULL REFERENCES media(id),
    position INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (post_id, media_id)
);

CREATE INDEX post_media_post_id_position_idx ON post_media (post_id, position);

CREATE TABLE hashtags (
    id  BLOB PRIMARY KEY NOT NULL,
    tag TEXT NOT NULL COLLATE NOCASE UNIQUE
);

CREATE TABLE post_hashtags (
    post_id    BLOB NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    hashtag_id BLOB NOT NULL REFERENCES hashtags(id) ON DELETE CASCADE,
    PRIMARY KEY (post_id, hashtag_id)
);

CREATE INDEX post_hashtags_hashtag_id_idx ON post_hashtags (hashtag_id);

CREATE TABLE follows (
    follower_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    followee_id BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now')),
    PRIMARY KEY (follower_id, followee_id)
);

CREATE INDEX follows_follower_id_idx ON follows (follower_id);
CREATE INDEX follows_followee_id_idx ON follows (followee_id);
