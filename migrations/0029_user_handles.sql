-- Handles are claimed once. Retired reservations are not recycled into a new
-- identity. No extension is needed for PostgreSQL's built-in SHA-256 function.
CREATE TABLE user_handles (
    handle_hash BYTEA PRIMARY KEY CHECK (octet_length(handle_hash) = 32),
    user_id UUID UNIQUE REFERENCES users(id) ON DELETE SET NULL,
    handle TEXT UNIQUE CHECK (handle ~ '^[a-z0-9_]{3,30}$'),
    CHECK ((user_id IS NULL) = (handle IS NULL)),
    CHECK (handle IS NULL OR handle_hash = sha256(convert_to(handle, 'UTF8')))
);

-- Account erasure retains only the digest needed to prevent impersonation,
-- without a plaintext handle, profile identifier or timestamp in the registry.
CREATE FUNCTION retire_user_handle() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.user_id IS NULL THEN
        NEW.handle := NULL;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER retire_user_handle_before_update
    BEFORE UPDATE OF user_id ON user_handles
    FOR EACH ROW EXECUTE FUNCTION retire_user_handle();
