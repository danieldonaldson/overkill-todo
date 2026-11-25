-- An unverified account cannot have a password set (email-first signup).
--
-- Added NOT VALID so it takes only a brief lock and checks new writes
-- immediately. The next migration validates existing rows without blocking
-- auth traffic.
ALTER TABLE users
    ADD CONSTRAINT users_verified_before_password
    CHECK (email_verified OR password_hash IS NULL)
    NOT VALID;
