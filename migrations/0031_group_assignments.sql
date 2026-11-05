SET LOCAL lock_timeout = '10s';
LOCK TABLE users,groups,group_memberships,workspace_content_pending,workspace_content_total IN SHARE ROW EXCLUSIVE MODE;

ALTER TABLE group_memberships ADD COLUMN progress_notice_version TEXT;
ALTER TABLE group_memberships ADD COLUMN progress_shared_at TIMESTAMPTZ;
ALTER TABLE group_memberships ADD CONSTRAINT group_membership_progress_identity UNIQUE(id,group_id,user_id,state);

CREATE TABLE group_assignments (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    title TEXT NOT NULL CHECK (length(trim(title)) BETWEEN 1 AND 240),
    description TEXT NOT NULL DEFAULT '' CHECK (length(description)<=10000),
    due_on DATE CHECK (due_on BETWEEN DATE '1900-01-01' AND DATE '9999-12-31'),
    due_time TIME,
    timezone TEXT NOT NULL CHECK (length(timezone) BETWEEN 1 AND 64),
    state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','closed')),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version>0),
    deleted_at TIMESTAMPTZ,
    undo_until TIMESTAMPTZ,
    deleted_membership_epoch BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(id,group_id),
    CHECK ((due_on IS NULL)=(due_time IS NULL)),
    CHECK ((deleted_at IS NULL AND undo_until IS NULL AND deleted_membership_epoch IS NULL)
        OR (deleted_at IS NOT NULL AND undo_until>deleted_at AND deleted_membership_epoch IS NOT NULL))
);
CREATE INDEX group_assignments_group_idx ON group_assignments(group_id,id);
CREATE INDEX group_assignments_lifecycle_idx ON group_assignments(group_id,state,id) WHERE deleted_at IS NULL;
CREATE INDEX group_assignments_cleanup_idx ON group_assignments(undo_until) WHERE deleted_at IS NOT NULL;

CREATE TABLE group_assignment_progress (
    group_id UUID NOT NULL,
    assignment_id UUID NOT NULL,
    membership_id UUID NOT NULL,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    membership_state TEXT NOT NULL DEFAULT 'active' CHECK (membership_state='active'),
    completed_at TIMESTAMPTZ,
    seen_assignment_version INTEGER CHECK (seen_assignment_version>0),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version>0),
    PRIMARY KEY(assignment_id,membership_id),
    FOREIGN KEY(assignment_id,group_id) REFERENCES group_assignments(id,group_id) ON DELETE CASCADE,
    FOREIGN KEY(membership_id,group_id,user_id,membership_state)
        REFERENCES group_memberships(id,group_id,user_id,state) ON DELETE CASCADE,
    CHECK ((completed_at IS NULL)=(seen_assignment_version IS NULL))
);
CREATE INDEX group_assignment_progress_user_idx ON group_assignment_progress(user_id,group_id);

-- Private preferences/progress must not advance public membership versions or
-- observable roster epochs. Public participation changes still guard Undo.
CREATE OR REPLACE FUNCTION advance_group_membership_epoch() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='UPDATE' AND (NEW.id,NEW.user_id,NEW.group_id,NEW.role,NEW.state)
        IS NOT DISTINCT FROM (OLD.id,OLD.user_id,OLD.group_id,OLD.role,OLD.state) THEN RETURN NEW; END IF;
    IF TG_OP='DELETE' THEN
        UPDATE groups SET membership_epoch=membership_epoch+1 WHERE id=OLD.group_id;
        RETURN OLD;
    END IF;
    UPDATE groups SET membership_epoch=membership_epoch+1 WHERE id=NEW.group_id;
    RETURN NEW;
END $$;
UPDATE group_memberships SET share_progress=FALSE WHERE share_progress;
ALTER TABLE group_memberships ADD CONSTRAINT progress_sharing_notice_check
    CHECK (NOT share_progress OR (progress_notice_version IS NOT NULL AND progress_shared_at IS NOT NULL AND length(progress_notice_version) BETWEEN 1 AND 40));
CREATE FUNCTION clear_departed_group_progress() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.state<>'active' OR NEW.id<>OLD.id THEN
        DELETE FROM group_assignment_progress WHERE membership_id=OLD.id;
        NEW.share_progress:=FALSE; NEW.progress_notice_version:=NULL; NEW.progress_shared_at:=NULL;
        NEW.discord_enabled:=FALSE; NEW.preferences_version:=OLD.preferences_version+1;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER clear_departed_group_progress BEFORE UPDATE ON group_memberships
    FOR EACH ROW EXECUTE FUNCTION clear_departed_group_progress();

-- Reserve one bounded progress slot for every assignment/active-member pair.
-- Progress remains sparse, not a fan-out of personal task copies. Once admitted,
-- members can complete/reopen existing work even when the content budget is full.
ALTER TABLE workspace_content_limits
    ADD COLUMN group_bytes BIGINT NOT NULL DEFAULT 16777216 CHECK (group_bytes>0),
    ADD COLUMN group_records BIGINT NOT NULL DEFAULT 20000 CHECK (group_records>0);
CREATE TABLE group_content_usage (
    group_id UUID PRIMARY KEY REFERENCES groups(id) ON DELETE CASCADE,
    bytes BIGINT NOT NULL DEFAULT 0 CHECK (bytes>=0),
    records BIGINT NOT NULL DEFAULT 0 CHECK (records>=0)
);
CREATE TABLE group_content_pending (
    transaction_id XID8 NOT NULL,
    group_id UUID NOT NULL,
    byte_delta BIGINT NOT NULL,
    record_delta BIGINT NOT NULL,
    PRIMARY KEY(transaction_id,group_id)
);
CREATE FUNCTION queue_group_content(group_key UUID,byte_change BIGINT,record_change BIGINT)
RETURNS VOID LANGUAGE plpgsql AS $$
BEGIN
    IF byte_change=0 AND record_change=0 THEN RETURN; END IF;
    INSERT INTO group_content_pending AS pending VALUES(pg_current_xact_id(),group_key,byte_change,record_change)
    ON CONFLICT(transaction_id,group_id) DO UPDATE
    SET byte_delta=pending.byte_delta+EXCLUDED.byte_delta,record_delta=pending.record_delta+EXCLUDED.record_delta;
END $$;
-- Cascades can remove both memberships and definitions before their AFTER
-- triggers observe one another. Release the scope's committed counter once,
-- overriding any uncommitted additions instead of guessing cascade order.
CREATE FUNCTION release_deleted_group_content() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE used_bytes BIGINT; used_records BIGINT;
BEGIN
    SELECT COALESCE(SUM(u.bytes),0),COALESCE(SUM(u.records),0) INTO used_bytes,used_records
        FROM group_content_usage u WHERE u.group_id=OLD.id;
    INSERT INTO group_content_pending AS pending VALUES(pg_current_xact_id(),OLD.id,-used_bytes,-used_records)
    ON CONFLICT(transaction_id,group_id) DO UPDATE
        SET byte_delta=EXCLUDED.byte_delta,record_delta=EXCLUDED.record_delta;
    RETURN OLD;
END $$;
CREATE TRIGGER release_deleted_group_content BEFORE DELETE ON groups
    FOR EACH ROW EXECUTE FUNCTION release_deleted_group_content();
-- 256 fixed lifecycle bytes cover versions/state/tombstones so closing/deleting
-- does not need new capacity. Variable content remains billed by serialized size.
CREATE FUNCTION group_definition_bytes(value JSONB) RETURNS BIGINT LANGUAGE SQL IMMUTABLE AS $$
    SELECT octet_length((value-ARRAY['version','state','deleted_at','undo_until','deleted_membership_epoch'])::text)::BIGINT+256
$$;
CREATE FUNCTION maintain_group_assignment_usage() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    group_key UUID; members BIGINT; byte_change BIGINT; record_change BIGINT;
BEGIN
    IF TG_OP='DELETE' THEN group_key:=OLD.group_id; ELSE group_key:=NEW.group_id; END IF;
    IF TG_OP='UPDATE' AND (NEW.group_id<>OLD.group_id OR NEW.id<>OLD.id) THEN
        RAISE EXCEPTION 'assignment identity cannot change' USING ERRCODE='23514';
    END IF;
    IF NOT EXISTS(SELECT 1 FROM groups WHERE id=group_key) THEN
        IF TG_OP='DELETE' THEN RETURN OLD; END IF; RETURN NEW;
    END IF;
    SELECT COUNT(*) INTO members FROM group_memberships WHERE group_id=group_key AND state='active';
    IF TG_OP='INSERT' THEN
        byte_change:=group_definition_bytes(to_jsonb(NEW))+members*512; record_change:=1+members;
    ELSIF TG_OP='DELETE' THEN
        byte_change:=-group_definition_bytes(to_jsonb(OLD))-members*512; record_change:=-1-members;
    ELSE
        byte_change:=group_definition_bytes(to_jsonb(NEW))-group_definition_bytes(to_jsonb(OLD)); record_change:=0;
    END IF;
    PERFORM queue_group_content(group_key,byte_change,record_change);
    IF TG_OP='DELETE' THEN RETURN OLD; END IF; RETURN NEW;
END $$;
CREATE TRIGGER group_assignment_usage AFTER INSERT OR UPDATE OR DELETE ON group_assignments
    FOR EACH ROW EXECUTE FUNCTION maintain_group_assignment_usage();
CREATE FUNCTION maintain_group_member_reservations() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    group_key UUID; member_delta INTEGER; assignments BIGINT;
BEGIN
    IF TG_OP='INSERT' THEN group_key:=NEW.group_id; member_delta:=(NEW.state='active')::INTEGER;
    ELSIF TG_OP='DELETE' THEN group_key:=OLD.group_id; member_delta:=-(OLD.state='active')::INTEGER;
    ELSE
        IF NEW.group_id<>OLD.group_id OR NEW.user_id<>OLD.user_id THEN
            RAISE EXCEPTION 'membership scope cannot change' USING ERRCODE='23514';
        END IF;
        group_key:=NEW.group_id;member_delta:=(NEW.state='active')::INTEGER-(OLD.state='active')::INTEGER;
    END IF;
    IF member_delta<>0 AND EXISTS(SELECT 1 FROM groups WHERE id=group_key) THEN
        SELECT COUNT(*) INTO assignments FROM group_assignments WHERE group_id=group_key;
        PERFORM queue_group_content(group_key,member_delta*assignments*512,member_delta*assignments);
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF; RETURN NEW;
END $$;
CREATE TRIGGER group_member_reservations AFTER INSERT OR UPDATE OR DELETE ON group_memberships
    FOR EACH ROW EXECUTE FUNCTION maintain_group_member_reservations();
CREATE FUNCTION bound_group_progress_row() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF octet_length(to_jsonb(NEW)::text)>512 THEN
        RAISE EXCEPTION 'progress exceeds reserved capacity' USING ERRCODE='23514',CONSTRAINT='workspace_content_quota';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER bound_group_progress_row BEFORE INSERT OR UPDATE ON group_assignment_progress
    FOR EACH ROW EXECUTE FUNCTION bound_group_progress_row();

-- Apply final net deltas from both scopes once per transaction, always locking
-- global then individual counters. Mixed deletion/Undo never needs spare global
-- capacity merely because deferred triggers happened to fire in a different order.
CREATE OR REPLACE FUNCTION apply_workspace_content_usage() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE
    xid XID8:=NEW.transaction_id;
    change RECORD; byte_change BIGINT; record_change BIGINT;
    current_bytes BIGINT; current_records BIGINT;
    total_bytes BIGINT; total_records BIGINT;
    limits workspace_content_limits%ROWTYPE;
BEGIN
    IF NOT EXISTS(SELECT 1 FROM workspace_content_pending WHERE transaction_id=xid)
        AND NOT EXISTS(SELECT 1 FROM group_content_pending WHERE transaction_id=xid) THEN RETURN NULL; END IF;
    SELECT COALESCE(SUM(byte_delta),0),COALESCE(SUM(record_delta),0) INTO byte_change,record_change
    FROM (SELECT byte_delta,record_delta FROM workspace_content_pending WHERE transaction_id=xid
          UNION ALL SELECT byte_delta,record_delta FROM group_content_pending WHERE transaction_id=xid) delta;
    UPDATE workspace_content_total SET bytes=bytes+byte_change,records=records+record_change
    WHERE singleton RETURNING bytes,records INTO total_bytes,total_records;
    SELECT * INTO limits FROM workspace_content_limits WHERE singleton;
    IF (byte_change>0 AND total_bytes>limits.total_bytes) OR (record_change>0 AND total_records>limits.total_records) THEN
        RAISE EXCEPTION 'workspace content limit reached' USING ERRCODE='23514',CONSTRAINT='workspace_content_quota';
    END IF;
    FOR change IN WITH removed AS (DELETE FROM workspace_content_pending WHERE transaction_id=xid RETURNING *)
        SELECT * FROM removed ORDER BY user_id LOOP
        IF EXISTS(SELECT 1 FROM users WHERE id=change.user_id) THEN
            INSERT INTO workspace_content_usage(user_id,bytes,records) VALUES(change.user_id,0,0) ON CONFLICT DO NOTHING;
            UPDATE workspace_content_usage SET bytes=bytes+change.byte_delta,records=records+change.record_delta
                WHERE user_id=change.user_id RETURNING bytes,records INTO current_bytes,current_records;
            IF (change.byte_delta>0 AND current_bytes>limits.user_bytes) OR (change.record_delta>0 AND current_records>limits.user_records) THEN
                RAISE EXCEPTION 'workspace content limit reached' USING ERRCODE='23514',CONSTRAINT='workspace_content_quota';
            END IF;
        END IF;
    END LOOP;
    FOR change IN WITH removed AS (DELETE FROM group_content_pending WHERE transaction_id=xid RETURNING *)
        SELECT * FROM removed ORDER BY group_id LOOP
        IF EXISTS(SELECT 1 FROM groups WHERE id=change.group_id) THEN
            INSERT INTO group_content_usage(group_id) VALUES(change.group_id) ON CONFLICT DO NOTHING;
            UPDATE group_content_usage SET bytes=bytes+change.byte_delta,records=records+change.record_delta
                WHERE group_id=change.group_id RETURNING bytes,records INTO current_bytes,current_records;
            IF (change.byte_delta>0 AND current_bytes>limits.group_bytes) OR (change.record_delta>0 AND current_records>limits.group_records) THEN
                RAISE EXCEPTION 'group content limit reached' USING ERRCODE='23514',CONSTRAINT='workspace_content_quota';
            END IF;
        END IF;
    END LOOP;
    RETURN NULL;
END $$;
CREATE CONSTRAINT TRIGGER group_content_apply AFTER INSERT OR UPDATE ON group_content_pending
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION apply_workspace_content_usage();
