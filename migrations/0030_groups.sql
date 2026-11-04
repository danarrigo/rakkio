-- Persist admission blocking before external account cleanup. A failed deletion
-- can be retried, but cannot acquire new group ownership after erasure begins.
ALTER TABLE users ADD COLUMN group_admission_blocked BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE groups (
    id UUID PRIMARY KEY,
    owner_id UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 120),
    timezone TEXT NOT NULL CHECK (length(timezone) BETWEEN 1 AND 64),
    approval_required BOOLEAN NOT NULL DEFAULT TRUE,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    membership_epoch BIGINT NOT NULL DEFAULT 0,
    deleted_at TIMESTAMPTZ,
    undo_until TIMESTAMPTZ,
    deleted_membership_epoch BIGINT,
    owner_state TEXT NOT NULL DEFAULT 'active' CHECK (owner_state = 'active'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK ((deleted_at IS NULL AND undo_until IS NULL AND deleted_membership_epoch IS NULL)
        OR (deleted_at IS NOT NULL AND undo_until > deleted_at AND deleted_membership_epoch IS NOT NULL))
);
CREATE INDEX groups_owner_idx ON groups(owner_id);
CREATE INDEX groups_deletion_idx ON groups(undo_until) WHERE deleted_at IS NOT NULL;

CREATE TABLE group_memberships (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('member','leader')),
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active','left','removed')),
    share_progress BOOLEAN NOT NULL DEFAULT FALSE,
    discord_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    preferences_version INTEGER NOT NULL DEFAULT 1 CHECK (preferences_version > 0),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    UNIQUE(group_id,user_id),
    UNIQUE(group_id,user_id,state),
    UNIQUE(id,group_id),
    UNIQUE(id,group_id,user_id),
    CHECK (state='active' OR (NOT share_progress AND NOT discord_enabled))
);
CREATE INDEX group_memberships_user_idx ON group_memberships(user_id,state);
CREATE INDEX group_memberships_group_idx ON group_memberships(group_id,state);
-- A group can never commit without exactly one active owner. Deferred because
-- group creation inserts the group and its owner membership in one transaction.
ALTER TABLE groups ADD CONSTRAINT groups_active_owner_fk
    FOREIGN KEY(id,owner_id,owner_state) REFERENCES group_memberships(group_id,user_id,state)
    DEFERRABLE INITIALLY DEFERRED;

CREATE FUNCTION advance_group_membership_epoch() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        UPDATE groups SET membership_epoch=membership_epoch+1 WHERE id=OLD.group_id;
        RETURN OLD;
    END IF;
    UPDATE groups SET membership_epoch=membership_epoch+1 WHERE id=NEW.group_id;
    RETURN NEW;
END;
$$;
CREATE TRIGGER group_membership_epoch_after_change
    AFTER INSERT OR UPDATE OR DELETE ON group_memberships
    FOR EACH ROW EXECUTE FUNCTION advance_group_membership_epoch();

CREATE TABLE group_invitations (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    issuer_id UUID REFERENCES users(id) ON DELETE SET NULL,
    kind TEXT NOT NULL CHECK (kind IN ('direct','link','code')),
    recipient_id UUID REFERENCES users(id) ON DELETE CASCADE,
    secret_hash BYTEA UNIQUE CHECK (octet_length(secret_hash)=32),
    expires_at TIMESTAMPTZ NOT NULL,
    max_uses INTEGER NOT NULL CHECK (max_uses BETWEEN 1 AND 100),
    uses INTEGER NOT NULL DEFAULT 0 CHECK (uses >= 0 AND uses <= max_uses),
    revoked_at TIMESTAMPTZ,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    UNIQUE(id,group_id),
    CHECK ((kind='direct' AND recipient_id IS NOT NULL AND secret_hash IS NULL AND max_uses=1)
        OR (kind IN ('link','code') AND recipient_id IS NULL AND secret_hash IS NOT NULL))
);
CREATE INDEX group_invitations_group_idx ON group_invitations(group_id);
CREATE INDEX group_invitations_recipient_idx ON group_invitations(recipient_id);
CREATE UNIQUE INDEX group_invitations_direct_idx ON group_invitations(group_id,recipient_id)
    WHERE kind='direct' AND revoked_at IS NULL AND uses=0;

CREATE TABLE group_join_requests (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    invitation_id UUID NOT NULL,
    invitation_version INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','denied','approved')),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(group_id,user_id),
    FOREIGN KEY(invitation_id,group_id) REFERENCES group_invitations(id,group_id) ON DELETE CASCADE
);
CREATE INDEX group_join_requests_user_idx ON group_join_requests(user_id);

CREATE TABLE group_ownership_offers (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL UNIQUE REFERENCES groups(id) ON DELETE CASCADE,
    from_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    to_membership_id UUID NOT NULL,
    group_version INTEGER NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    FOREIGN KEY(to_membership_id,group_id) REFERENCES group_memberships(id,group_id) ON DELETE CASCADE,
    FOREIGN KEY(group_id,from_user_id) REFERENCES group_memberships(group_id,user_id) ON DELETE CASCADE
);
