-- id is actix-session's session key: a 64-char alphanumeric token, hence TEXT.
CREATE TABLE sessions (
    id           TEXT PRIMARY KEY,
    -- NULL for anonymous sessions; set once login plants user_id in the state
    user_id      UUID REFERENCES users(id) ON DELETE CASCADE,
    state        JSONB NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ,
    last_seen_at TIMESTAMPTZ,
    user_agent   TEXT,
    ip           INET
);

CREATE INDEX sessions_user_id_idx    ON sessions (user_id);
CREATE INDEX sessions_expires_at_idx ON sessions (expires_at);
