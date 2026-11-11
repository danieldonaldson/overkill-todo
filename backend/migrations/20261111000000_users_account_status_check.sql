-- A deleted account cannot also be pending deletion.
--
-- The app reads (deleted_at, deletion_scheduled_at) as one AccountStatus, where
-- deletion is terminal; see domain/value_objects/account_status.rs. Until the
-- release that introduced that type, a request to delete an already-anonymized
-- account could write both columns. This is the contract step that follows it:
-- it must ship in a LATER deploy than that code change, since the code before it
-- can still violate the constraint (migrations/README.md).
--
-- Clear any schedule left beside a deletion (deleted_at already wins when the
-- app reads the row, so no user-visible change), then add the constraint
-- NOT VALID: that only takes a brief lock and checks new writes from here on.
-- The next migration validates existing rows without blocking writes.
UPDATE users
SET deletion_scheduled_at = NULL
WHERE deleted_at IS NOT NULL AND deletion_scheduled_at IS NOT NULL;

ALTER TABLE users
    ADD CONSTRAINT users_deleted_not_pending_deletion
    CHECK (deleted_at IS NULL OR deletion_scheduled_at IS NULL)
    NOT VALID;
