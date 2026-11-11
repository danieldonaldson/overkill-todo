-- Validate users_deleted_not_pending_deletion against existing rows.
--
-- A separate migration (so a separate transaction) from the ADD CONSTRAINT:
-- VALIDATE takes only a SHARE UPDATE EXCLUSIVE lock, so the full-table scan
-- does not block reads or writes. In the same transaction it would run under
-- the ADD's ACCESS EXCLUSIVE lock and block auth traffic for the whole scan.
ALTER TABLE users VALIDATE CONSTRAINT users_deleted_not_pending_deletion;
