-- Initial schema: users and the auth slice.
--
-- Runs inside one transaction against empty tables, so plain `CREATE INDEX` is
-- fine here. Indexes added later on populated tables need `CONCURRENTLY` and a
-- `-- no-transaction` migration; see README.md before adding one.

-- pg_stat_statements powers query performance monitoring.
-- Note: shared_preload_libraries must include 'pg_stat_statements' in postgresql.conf
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;

-- UUID primary keys use the native uuidv7() function, available in PostgreSQL 18+.

-- =============================================================================
-- ENUMS
-- =============================================================================

CREATE TYPE oauth_provider AS ENUM ('google', 'apple');
CREATE TYPE user_role AS ENUM ('user', 'admin', 'super_admin');

-- =============================================================================
-- HELPER FUNCTIONS
-- =============================================================================

-- Trigger function to auto-update updated_at column
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ language 'plpgsql';

-- =============================================================================
-- USERS TABLE
-- =============================================================================

CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    email VARCHAR(255) UNIQUE NOT NULL,
    password_hash VARCHAR(255),  -- nullable for OAuth-only users
    first_name VARCHAR(100) NOT NULL DEFAULT '',
    last_name VARCHAR(100) NOT NULL DEFAULT '',
    email_verified BOOLEAN NOT NULL DEFAULT false,

    -- Email change support
    pending_email VARCHAR(255),
    pending_email_requested_at TIMESTAMPTZ,

    -- Soft delete support
    deleted_at TIMESTAMPTZ,
    deletion_scheduled_at TIMESTAMPTZ,

    -- Activity tracking. Written only by the `refresh_token_updates_last_active`
    -- trigger, which always supplies NOW() — never null.
    last_active_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Role-based access control. Every user registers as 'user'. Bootstrap the
    -- first admin out-of-band with `just grant-admin <email>`.
    role user_role NOT NULL DEFAULT 'user',

    -- Optimistic-concurrency guard. Every user write goes read-modify-write
    -- through `find_by_id` -> mutate -> `save`. Without this, two concurrent
    -- flows (e.g. an email verification racing an account-deletion schedule)
    -- clobber each other's column changes, silently resurrecting scrubbed PII or
    -- undoing a verification. `save` applies a conditional
    -- `... WHERE version = $expected` guard and bumps the version, so a stale
    -- write is rejected (0 rows) instead of overwriting a concurrent change.
    version BIGINT NOT NULL DEFAULT 0,

    -- Token-invalidation cutoff. A session JWT carries its issued-at (`iat`); a
    -- token whose `iat` is strictly before this instant is no longer honored on
    -- sensitive (admin) routes. A role change sets this to NOW() so a demotion
    -- revokes outstanding admin access immediately instead of waiting for the
    -- short-lived access token to expire. NULL (the default) means "no cutoff".
    tokens_valid_after TIMESTAMPTZ,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Note: idx_users_email is not needed as UNIQUE constraint on email already creates an index
CREATE INDEX idx_users_pending_email ON users (pending_email) WHERE pending_email IS NOT NULL;
CREATE INDEX idx_users_role ON users(role) WHERE role != 'user';

-- The account purger only needs non-deleted scheduled rows.
CREATE INDEX idx_users_deletion_scheduled_at
    ON users (deletion_scheduled_at)
    WHERE deletion_scheduled_at IS NOT NULL AND deleted_at IS NULL;

-- Admin pagination filters deleted users and uses id as its deterministic
-- tiebreaker. Include both so PostgreSQL can serve pages in order.
CREATE INDEX idx_users_active_created_at
    ON users (created_at DESC, id DESC)
    WHERE deleted_at IS NULL;

CREATE TRIGGER update_users_updated_at BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

COMMENT ON COLUMN users.deleted_at IS 'When the account was permanently soft-deleted (cannot be recovered)';
COMMENT ON COLUMN users.deletion_scheduled_at IS 'When the account is scheduled for permanent deletion (14 days from request)';

-- =============================================================================
-- REFRESH TOKENS TABLE
-- =============================================================================

CREATE TABLE refresh_tokens (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash VARCHAR(64) NOT NULL,  -- SHA-256 hash of the token (64 hex chars)
    device_fingerprint VARCHAR(64) NOT NULL,  -- SHA-256 hash of device info
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,  -- NULL = active, timestamp = revoked
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- The token hash is the primary auth probe and is logically unique (each rotation
-- mints a fresh random token); a UNIQUE index both speeds the lookup and makes a
-- hash collision / double-insert fail loudly rather than silently duplicate.
CREATE UNIQUE INDEX idx_refresh_tokens_hash ON refresh_tokens(token_hash);
-- "List active sessions" filters user_id AND revoked_at IS NULL; a partial index
-- keeps it tight as revoked rows accumulate (audit trail is never deleted).
CREATE INDEX idx_refresh_tokens_user_id_active ON refresh_tokens(user_id) WHERE revoked_at IS NULL;
CREATE INDEX idx_refresh_tokens_expires_at ON refresh_tokens(expires_at);
-- Foreign-key actions and account anonymization/deletion need every child row,
-- not just the live rows covered by the partial index above.
CREATE INDEX idx_refresh_tokens_user_id ON refresh_tokens (user_id);

COMMENT ON TABLE refresh_tokens IS 'Stores hashed refresh tokens. Tokens are single-use (rotated on each refresh). Old tokens are kept for audit trail but marked revoked.';
COMMENT ON COLUMN refresh_tokens.token_hash IS 'SHA-256 hash of the refresh token - never store plaintext tokens';
COMMENT ON COLUMN refresh_tokens.device_fingerprint IS 'SHA-256 hash of device characteristics - must match on refresh to prevent stolen token usage';
COMMENT ON COLUMN refresh_tokens.revoked_at IS 'NULL means token is active; timestamp means when it was revoked';

-- Function to update last_active_at when refresh token is created
CREATE FUNCTION update_user_last_active() RETURNS TRIGGER AS $$
BEGIN
    UPDATE users SET last_active_at = NOW() WHERE id = NEW.user_id;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER refresh_token_updates_last_active
    AFTER INSERT ON refresh_tokens
    FOR EACH ROW EXECUTE FUNCTION update_user_last_active();

-- =============================================================================
-- OAUTH ACCOUNTS TABLE
-- =============================================================================

CREATE TABLE user_oauth_accounts (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider oauth_provider NOT NULL,
    provider_user_id VARCHAR(255) NOT NULL,
    provider_email VARCHAR(255),
    linked_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(provider, provider_user_id),
    UNIQUE(user_id, provider)
);

CREATE INDEX idx_user_oauth_accounts_user_id ON user_oauth_accounts(user_id);

COMMENT ON TABLE user_oauth_accounts IS 'OAuth provider accounts linked to users';
COMMENT ON COLUMN user_oauth_accounts.provider IS 'OAuth provider: google, apple';
COMMENT ON COLUMN user_oauth_accounts.provider_user_id IS 'sub claim from provider ID token';

-- =============================================================================
-- EMAIL VERIFICATION CODES TABLE
-- =============================================================================

CREATE TABLE email_verification_codes (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL,
    magic_token_hash TEXT,  -- Alternative to 6-digit codes for web-only backup
    expires_at TIMESTAMPTZ NOT NULL,

    -- The magic link gets its own expiry rather than sharing the code's
    -- `expires_at`. A routine code resend refreshes `expires_at`, which would
    -- otherwise silently extend a previously-issued link's lifetime (and keep an
    -- old link valid against a superseded code).
    magic_token_expires_at TIMESTAMPTZ,

    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_sent_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT email_verification_codes_user_id_unique UNIQUE (user_id)
);

-- Note: idx_email_verification_codes_user_id not needed as UNIQUE(user_id) constraint creates an index
CREATE INDEX idx_email_verification_codes_expires_at ON email_verification_codes(expires_at);
CREATE INDEX idx_email_verification_codes_magic_token ON email_verification_codes(magic_token_hash) WHERE magic_token_hash IS NOT NULL;

-- =============================================================================
-- PASSWORD RESET CODES TABLE
-- =============================================================================

CREATE TABLE password_reset_codes (
    id UUID PRIMARY KEY DEFAULT uuidv7(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    -- Mirrors email_verification_codes so the password-reset flow can enforce a
    -- resend cooldown and a brute-force lockout window measured from the last
    -- send. Without it, re-requesting a reset upserts a fresh attempts=0 code and
    -- silently clears an active lockout.
    last_sent_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT password_reset_codes_user_id_unique UNIQUE (user_id)
);

-- Note: idx_password_reset_codes_user_id not needed as UNIQUE(user_id) constraint creates an index
CREATE INDEX idx_password_reset_codes_expires_at ON password_reset_codes(expires_at);
