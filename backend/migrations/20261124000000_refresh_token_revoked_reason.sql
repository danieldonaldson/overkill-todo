-- Record why a refresh token was revoked so reuse detection only fires on
-- theft signals (replaying a rotated token or a legacy/reuse-revoked token),
-- not benign replays of a logged-out or password-change-evicted session.
--
-- Expand-safe: nullable column with a CHECK constraint. Older pods ignore it
-- and leave it NULL; the app treats NULL on a revoked row as `Unknown`, which
-- still triggers reuse detection (fail closed).
ALTER TABLE refresh_tokens
    ADD COLUMN revoked_reason TEXT
        CONSTRAINT refresh_tokens_revoked_reason_check
        CHECK (revoked_reason IN ('rotated', 'logout', 'reuse_detected', 'password_change'));

COMMENT ON COLUMN refresh_tokens.revoked_reason IS
    'Why the token was revoked (rotated, logout, reuse_detected, password_change). NULL on active tokens or legacy revocations.';
