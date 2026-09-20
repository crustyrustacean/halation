CREATE TABLE hashtags (
    id  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tag CITEXT NOT NULL UNIQUE
);

CREATE TABLE post_hashtags (
    post_id    UUID NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    hashtag_id UUID NOT NULL REFERENCES hashtags(id) ON DELETE CASCADE,
    PRIMARY KEY (post_id, hashtag_id)
);
CREATE INDEX post_hashtags_hashtag_id_idx ON post_hashtags (hashtag_id);
