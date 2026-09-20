CREATE TABLE posts (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID NOT NULL REFERENCES users(id),
    caption       TEXT,
    location_name TEXT,
    lat           DOUBLE PRECISION,
    lng           DOUBLE PRECISION,
    filter        TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX posts_user_id_created_at_idx ON posts (user_id, created_at DESC);
CREATE INDEX posts_created_at_idx ON posts (created_at DESC);

CREATE TABLE post_media (
    post_id  UUID NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    media_id UUID NOT NULL REFERENCES media(id),
    position INT NOT NULL DEFAULT 0,
    PRIMARY KEY (post_id, media_id)
);
CREATE INDEX post_media_post_id_position_idx ON post_media (post_id, position);
