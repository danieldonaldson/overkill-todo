-- Escalating brute-force lockout for the 6-digit verification and reset codes.
--
-- `lockouts` counts how many codes for this user were replaced while locked at
-- the attempt cap. Each one doubles the next lock window (15 min, 30 min, 1 h, …,
-- capped at 24 h), so a sustained guessing campaign slows to a few guesses a day
-- instead of 5 every 15 minutes. The counter survives code reissues (the upserts
-- leave it alone, or bump it) and is forgotten only when the user succeeds or the
-- row has been idle for 30 days (see database_maintenance.rs).
--
-- Expand-only: old code never names the column, and the default covers its inserts.
ALTER TABLE email_verification_codes
    ADD COLUMN lockouts INTEGER NOT NULL DEFAULT 0 CHECK (lockouts >= 0);

ALTER TABLE password_reset_codes
    ADD COLUMN lockouts INTEGER NOT NULL DEFAULT 0 CHECK (lockouts >= 0);
