-- Existing files remain ready. New uploads reserve quota before external I/O,
-- but are not published until confirmed. A separate table preserves existing
-- FileRecord/export/Undo snapshot formats and keeps failed writes charged.
CREATE TABLE pending_file_uploads (
    file_id UUID PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (clock_timestamp()+INTERVAL '5 minutes'),
    cleanup_after TIMESTAMPTZ NOT NULL DEFAULT (clock_timestamp()+INTERVAL '5 minutes')
);
CREATE INDEX pending_file_uploads_cleanup_idx ON pending_file_uploads(cleanup_after);
CREATE INDEX oauth_login_attempts_browser_idx ON oauth_login_attempts(browser_binding_hash);
