CREATE EXTENSION IF NOT EXISTS citext;

CREATE TABLE users (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username      CITEXT    NOT NULL CHECK (username ~ '^[a-z0-9_]{3,30}$'),
    email         CITEXT    NOT NULL,
    password_hash TEXT      NOT NULL,
    display_name  TEXT,
    bio           TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at    TIMESTAMPTZ
);

-- uniqueness among live accounts only; handles/emails recycle after deletion
CREATE UNIQUE INDEX users_username_live_idx ON users (username) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX users_email_live_idx    ON users (email)    WHERE deleted_at IS NULL;
