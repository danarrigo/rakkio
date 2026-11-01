-- Discord identities are optional, private, and local to this installation.
CREATE TABLE discord_connections (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    discord_user_id TEXT NOT NULL UNIQUE CHECK (discord_user_id ~ '^[0-9]{1,20}$'),
    username TEXT NOT NULL CHECK (octet_length(username) BETWEEN 1 AND 256),
    timezone TEXT NOT NULL CHECK (octet_length(timezone) BETWEEN 1 AND 128),
    consent_version TEXT NOT NULL,
    connected_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE discord_oauth_attempts (
    state_hash BYTEA PRIMARY KEY,
    browser_binding_hash BYTEA NOT NULL,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    pkce_verifier TEXT NOT NULL,
    timezone TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL DEFAULT NOW() + INTERVAL '10 minutes'
);
CREATE INDEX discord_oauth_expiry_idx ON discord_oauth_attempts(expires_at);

-- No Discord access/refresh tokens or interaction tokens are persisted.
-- Receipts and task changes commit together, so forwarding retries cannot repeat
-- creation/completion. At most 100 unexpired receipts per connected account.
CREATE TABLE discord_interactions (
    id TEXT PRIMARY KEY CHECK (id ~ '^[0-9]{1,20}$'),
    connection_id UUID NOT NULL REFERENCES discord_connections(id) ON DELETE CASCADE,
    response JSONB NOT NULL CHECK (octet_length(response::text) <= 8192),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX discord_interactions_connection_idx ON discord_interactions(connection_id, created_at);
CREATE INDEX discord_interactions_expiry_idx ON discord_interactions(created_at);
