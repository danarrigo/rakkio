-- Owner-only Edunex (ITB LMS) assignment import. The pasted bearer token is
-- encrypted with the same key as Google credentials.
CREATE TABLE edunex_connections (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    encrypted_token BYTEA NOT NULL,
    token_expires_at TIMESTAMPTZ,
    last_attempt_at TIMESTAMPTZ,
    last_synced_at TIMESTAMPTZ,
    last_error TEXT CHECK (char_length(last_error) <= 500),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One row per imported Edunex assignment. task_id becomes NULL when the user
-- deletes the rakkio task, which keeps it from being imported again.
CREATE TABLE edunex_task_links (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    edunex_task_id TEXT NOT NULL CHECK (char_length(edunex_task_id) BETWEEN 1 AND 64),
    task_id UUID UNIQUE REFERENCES tasks(id) ON DELETE SET NULL,
    synced_title TEXT NOT NULL,
    synced_due_at TIMESTAMPTZ NOT NULL,
    synced_submitted BOOLEAN NOT NULL,
    PRIMARY KEY (user_id, edunex_task_id)
);
