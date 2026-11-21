-- Validates the widened kind check added NOT VALID by the previous migration.
-- VALIDATE takes only a SHARE UPDATE EXCLUSIVE lock, so writes continue.
ALTER TABLE audit_events VALIDATE CONSTRAINT audit_events_kind_check;
