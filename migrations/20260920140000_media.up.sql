CREATE TABLE media (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id    UUID NOT NULL REFERENCES users(id),
    kind        TEXT NOT NULL DEFAULT 'photo',   -- 'photo' | 'video' (later)
    storage_key TEXT NOT NULL,                   -- "{id}/original.{ext}"
    mime_type   TEXT NOT NULL,
    width       INT NOT NULL,
    height      INT NOT NULL,
    size_bytes  BIGINT NOT NULL,
    sha256      TEXT NOT NULL,
    -- display-only EXIF copied out of the original; served derivatives are
    -- stripped, so this DB copy is the only EXIF that exists anywhere
    exif        JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX media_owner_id_idx ON media (owner_id);

CREATE TABLE media_derivatives (
    media_id    UUID NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    variant     TEXT NOT NULL,                   -- 'thumb' | 'medium' | 'large'
    storage_key TEXT NOT NULL,
    width       INT NOT NULL,
    height      INT NOT NULL,
    size_bytes  BIGINT NOT NULL,
    PRIMARY KEY (media_id, variant)
);

ALTER TABLE users ADD COLUMN avatar_media_id UUID REFERENCES media(id);
