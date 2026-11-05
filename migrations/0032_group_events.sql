SET LOCAL lock_timeout='10s';
LOCK TABLE users,groups IN SHARE ROW EXCLUSIVE MODE;

-- Canonical group definitions, never personal calendar/event copies. Local civil
-- times plus an explicit zone preserve recurring wall time across DST changes.
CREATE TABLE group_events (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    title TEXT NOT NULL CHECK(length(trim(title)) BETWEEN 1 AND 240),
    description TEXT NOT NULL DEFAULT '' CHECK(length(description)<=10000),
    location TEXT NOT NULL DEFAULT '' CHECK(length(location)<=500),
    starts_local TIMESTAMP NOT NULL,
    ends_local TIMESTAMP NOT NULL,
    all_day BOOLEAN NOT NULL DEFAULT FALSE,
    timezone TEXT NOT NULL CHECK(length(timezone) BETWEEN 1 AND 64),
    recurrence TEXT NOT NULL DEFAULT 'none' CHECK(recurrence IN ('none','daily','weekly','monthly','yearly')),
    recurrence_until DATE,
    version INTEGER NOT NULL DEFAULT 1 CHECK(version>0),
    deleted_at TIMESTAMPTZ,
    undo_until TIMESTAMPTZ,
    deleted_membership_epoch BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(id,group_id),
    CHECK(starts_local>=TIMESTAMP '1900-01-01' AND ends_local<TIMESTAMP '9999-01-01'),
    CHECK(ends_local>starts_local AND ends_local-starts_local<=INTERVAL '31 days'),
    CHECK(NOT all_day OR (starts_local::time=TIME '00:00' AND ends_local::time=TIME '00:00')),
    CHECK((recurrence_until IS NULL OR recurrence_until>=starts_local::date)
        AND (recurrence<>'none' OR recurrence_until IS NULL)
        AND (recurrence_until IS NULL OR recurrence_until<DATE '9999-01-01')),
    CHECK((deleted_at IS NULL AND undo_until IS NULL AND deleted_membership_epoch IS NULL)
        OR (deleted_at IS NOT NULL AND undo_until>deleted_at AND deleted_membership_epoch IS NOT NULL))
);
CREATE INDEX group_events_group_idx ON group_events(group_id,id);
CREATE INDEX group_events_active_idx ON group_events(group_id,starts_local,id) WHERE deleted_at IS NULL;
CREATE INDEX group_events_cleanup_idx ON group_events(undo_until) WHERE deleted_at IS NOT NULL;

-- Same bounded group/global accounting, fixed lifecycle allowance and final-net
-- application as assignments. No per-member progress records exist for events.
-- New table starts empty, so existing tracked row shapes/counters need no recount.
CREATE FUNCTION maintain_group_event_usage() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE group_key UUID;byte_change BIGINT;record_change BIGINT;
BEGIN
    IF TG_OP='DELETE' THEN group_key:=OLD.group_id;ELSE group_key:=NEW.group_id;END IF;
    IF TG_OP='UPDATE' AND (NEW.id<>OLD.id OR NEW.group_id<>OLD.group_id) THEN
        RAISE EXCEPTION 'event identity cannot change' USING ERRCODE='23514';
    END IF;
    -- Parent deletion already releases its entire scope counter once, including
    -- pending additions. Do not depend on FK cascade ordering for credits.
    IF NOT EXISTS(SELECT 1 FROM groups WHERE id=group_key) THEN
        IF TG_OP='DELETE' THEN RETURN OLD;END IF;RETURN NEW;
    END IF;
    IF TG_OP='INSERT' THEN byte_change:=group_definition_bytes(to_jsonb(NEW));record_change:=1;
    ELSIF TG_OP='DELETE' THEN byte_change:=-group_definition_bytes(to_jsonb(OLD));record_change:=-1;
    ELSE byte_change:=group_definition_bytes(to_jsonb(NEW))-group_definition_bytes(to_jsonb(OLD));record_change:=0;
    END IF;
    PERFORM queue_group_content(group_key,byte_change,record_change);
    IF TG_OP='DELETE' THEN RETURN OLD;END IF;RETURN NEW;
END $$;
CREATE TRIGGER group_event_usage AFTER INSERT OR UPDATE OR DELETE ON group_events
    FOR EACH ROW EXECUTE FUNCTION maintain_group_event_usage();
