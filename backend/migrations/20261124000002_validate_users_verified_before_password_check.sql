-- Validate users_verified_before_password against existing rows.
--
-- Separate migration (separate transaction) so VALIDATE takes only a
-- SHARE UPDATE EXCLUSIVE lock and does not block reads or writes.
ALTER TABLE users VALIDATE CONSTRAINT users_verified_before_password;
