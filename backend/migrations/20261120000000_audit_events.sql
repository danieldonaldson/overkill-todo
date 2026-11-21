-- OT-008: the audit log. A permanent, append-only record of privileged and
-- security-relevant account actions.
--
-- Additive: the previous release never touches this table, so the migration is
-- safe mid-roll. Rollback is redeploying the previous digest; the table stays.
--
-- Events identify people by account ID only (no FK: an event must outlive any
-- row it names). `details` holds only closed, non-personal values (for a role
-- change the old and new role); the domain type allows nothing free-form.
-- Later slices widen the `kind` CHECK, which only accepts more values and so is
-- safe with the previous release.
CREATE TABLE audit_events (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    kind        TEXT NOT NULL CONSTRAINT audit_events_kind_check
                    CHECK (kind IN ('role_changed')),
    subject_id  UUID NOT NULL,
    actor_kind  TEXT NOT NULL CONSTRAINT audit_events_actor_kind_check
                    CHECK (actor_kind IN ('user', 'system', 'operator')),
    actor_id    UUID,
    details     JSONB NOT NULL DEFAULT '{}'::jsonb
                    CONSTRAINT audit_events_details_object_check
                    CHECK (jsonb_typeof(details) = 'object'),
    trace_id    TEXT,
    CONSTRAINT audit_events_actor_id_check
        CHECK ((actor_kind = 'user') = (actor_id IS NOT NULL))
);

-- Pages are ordered by `id DESC`: the identity follows insert order, and the
-- insert happens inside the transaction that applies the action.
CREATE INDEX audit_events_subject_idx ON audit_events (subject_id, id DESC);
CREATE INDEX audit_events_actor_idx ON audit_events (actor_id, id DESC)
    WHERE actor_id IS NOT NULL;
CREATE INDEX audit_events_kind_idx ON audit_events (kind, id DESC);

-- Append-only guard. The app owns this table (it runs the migrations), so a
-- REVOKE alone wouldn't hold: the owner could grant itself the right back.
-- Triggers refuse the change in the database itself. The one exception is the
-- retention policy: a row older than 7 years may be deleted. Dropping the
-- trigger is still possible for the owner; that would be a reviewed migration.
CREATE FUNCTION audit_events_refuse_change() RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'DELETE' AND OLD.occurred_at < now() - interval '7 years' THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'audit_events is append-only (% refused)', TG_OP
        USING ERRCODE = 'insufficient_privilege';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER audit_events_append_only
    BEFORE UPDATE OR DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION audit_events_refuse_change();

CREATE FUNCTION audit_events_refuse_truncate() RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'audit_events is append-only (TRUNCATE refused)'
        USING ERRCODE = 'insufficient_privilege';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER audit_events_no_truncate
    BEFORE TRUNCATE ON audit_events
    FOR EACH STATEMENT EXECUTE FUNCTION audit_events_refuse_truncate();
