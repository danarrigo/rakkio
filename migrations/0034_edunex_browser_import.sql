-- Edunex refuses connections from hosted servers, so the owner's browser now
-- fetches assignments and posts them to rakkio. The server keeps no token.
ALTER TABLE edunex_connections
    DROP COLUMN encrypted_token,
    DROP COLUMN token_expires_at,
    DROP COLUMN last_attempt_at,
    DROP COLUMN last_error;
