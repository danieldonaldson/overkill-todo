//! `PostgreSQL` implementation of the email verification repository.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;

use crate::application::ports::EmailVerificationRepository;
use crate::domain::entities::EmailVerificationCode;
use crate::domain::entities::attempt_lockout::AttemptOutcome;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::{UserId, VerificationCodeId};
use crate::infrastructure::persistence::postgres::connection::Database;
use crate::infrastructure::persistence::postgres::error::persist_err;
use crate::infrastructure::persistence::postgres::models::EmailVerificationCodeRow;

pub struct PostgresEmailVerificationRepository {
    db: Arc<Database>,
}

impl PostgresEmailVerificationRepository {
    #[must_use]
    pub const fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn row_to_entity(row: EmailVerificationCodeRow) -> EmailVerificationCode {
        EmailVerificationCode::from_persistence(
            VerificationCodeId::from_uuid(row.id),
            UserId::from_uuid(row.user_id),
            row.code_hash,
            row.expires_at,
            row.attempts,
            row.last_sent_at,
            row.created_at,
            row.magic_token_hash,
            row.magic_token_expires_at,
        )
    }
}

#[async_trait]
impl EmailVerificationRepository for PostgresEmailVerificationRepository {
    #[tracing::instrument(name = "repo.email_verification.save", skip(self, code), fields(user_id = %code.user_id()))]
    async fn save(&self, code: &EmailVerificationCode) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            INSERT INTO email_verification_codes (id, user_id, code_hash, expires_at, attempts, last_sent_at, created_at, magic_token_hash, magic_token_expires_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (user_id) DO UPDATE SET
                id = EXCLUDED.id,
                code_hash = EXCLUDED.code_hash,
                expires_at = EXCLUDED.expires_at,
                attempts = EXCLUDED.attempts,
                last_sent_at = EXCLUDED.last_sent_at,
                -- A routine 6-digit code-send carries magic_token_hash = NULL;
                -- only overwrite when a new magic token is actually supplied so a
                -- previously issued magic link stays matchable. The link's expiry
                -- is preserved alongside it so a code resend never extends it.
                magic_token_hash = COALESCE(EXCLUDED.magic_token_hash, email_verification_codes.magic_token_hash),
                magic_token_expires_at = COALESCE(EXCLUDED.magic_token_expires_at, email_verification_codes.magic_token_expires_at)
            "#,
            code.id().as_uuid(),
            code.user_id().as_uuid(),
            code.code_hash(),
            code.expires_at(),
            code.attempts(),
            code.last_sent_at(),
            code.created_at(),
            code.magic_token_hash(),
            code.magic_token_expires_at(),
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.email_verification.find_by_user", skip(self), fields(user_id = %user_id))]
    async fn find_by_user(
        &self,
        user_id: &UserId,
    ) -> Result<Option<EmailVerificationCode>, DomainError> {
        let row = sqlx::query_as!(
            EmailVerificationCodeRow,
            r#"
            SELECT id, user_id, code_hash, expires_at, attempts, last_sent_at, created_at, magic_token_hash, magic_token_expires_at
            FROM email_verification_codes
            WHERE user_id = $1
            "#,
            user_id.as_uuid()
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(row.map(Self::row_to_entity))
    }

    #[tracing::instrument(name = "repo.email_verification.delete_for_user", skip(self), fields(user_id = %user_id))]
    async fn delete_for_user(&self, user_id: &UserId) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            DELETE FROM email_verification_codes
            WHERE user_id = $1
            "#,
            user_id.as_uuid()
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.email_verification.register_attempt", skip(self), fields(user_id = %user_id))]
    async fn register_attempt(
        &self,
        user_id: &UserId,
        max_attempts: i32,
    ) -> Result<AttemptOutcome, DomainError> {
        // One atomic statement: the CTE claims a slot while below the cap, and
        // the outer SELECT reports whether a row exists at all, so callers can
        // tell "locked" (row present, at cap) from "no row" (deleted/expired
        // concurrently) without a second round trip or a misleading message.
        let row = sqlx::query!(
            r#"
            WITH bumped AS (
                UPDATE email_verification_codes
                SET attempts = attempts + 1
                WHERE user_id = $1 AND attempts < $2
                RETURNING attempts
            )
            SELECT
                (SELECT attempts FROM bumped) AS "bumped_attempts?: i32",
                EXISTS (SELECT 1 FROM email_verification_codes WHERE user_id = $1) AS "row_exists!"
            "#,
            user_id.as_uuid(),
            max_attempts,
        )
        .fetch_one(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(match row.bumped_attempts {
            Some(attempts) => AttemptOutcome::Claimed(attempts),
            None if row.row_exists => AttemptOutcome::Locked,
            None => AttemptOutcome::NotFound,
        })
    }

    #[tracing::instrument(name = "repo.email_verification.release_attempt", skip(self), fields(user_id = %user_id))]
    async fn release_attempt(&self, user_id: &UserId) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            UPDATE email_verification_codes
            SET attempts = GREATEST(attempts - 1, 0)
            WHERE user_id = $1
            "#,
            user_id.as_uuid(),
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.email_verification.set_last_sent_at", skip(self), fields(user_id = %user_id))]
    async fn set_last_sent_at(
        &self,
        user_id: &UserId,
        last_sent_at: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            UPDATE email_verification_codes
            SET last_sent_at = $2
            WHERE user_id = $1
            "#,
            user_id.as_uuid(),
            last_sent_at,
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.email_verification.update_for_resend", skip(self, code_hash), fields(user_id = %user_id))]
    async fn update_for_resend(
        &self,
        user_id: &UserId,
        code_hash: String,
        expires_at: DateTime<Utc>,
    ) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            UPDATE email_verification_codes
            SET code_hash = $2, expires_at = $3, attempts = 0, last_sent_at = NOW()
            WHERE user_id = $1
            "#,
            user_id.as_uuid(),
            code_hash,
            expires_at,
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.email_verification.delete_expired", skip(self))]
    async fn delete_expired(&self) -> Result<u64, DomainError> {
        let result = sqlx::query!(
            r#"
            DELETE FROM email_verification_codes
            WHERE expires_at < NOW()
            "#,
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(result.rows_affected())
    }

    #[tracing::instrument(name = "repo.email_verification.save_magic_token", skip(self, magic_token_hash), fields(user_id = %user_id))]
    async fn save_magic_token(
        &self,
        user_id: &UserId,
        magic_token_hash: String,
    ) -> Result<(), DomainError> {
        // Give the magic link its own lifetime (decoupled from the 6-digit
        // code's expiry) derived from the single source of truth, so this path
        // can't silently diverge from the entity's `EXPIRY_HOURS`.
        let magic_token_expires_at = Utc::now()
            + chrono::Duration::hours(
                crate::domain::entities::email_verification_code::EXPIRY_HOURS,
            );
        sqlx::query!(
            r#"
            UPDATE email_verification_codes
            SET magic_token_hash = $2,
                magic_token_expires_at = $3
            WHERE user_id = $1
            "#,
            user_id.as_uuid(),
            magic_token_hash,
            magic_token_expires_at,
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(
        name = "repo.email_verification.find_by_magic_token_hash",
        skip(self, magic_token_hash)
    )]
    async fn find_by_magic_token_hash(
        &self,
        magic_token_hash: &str,
    ) -> Result<Option<EmailVerificationCode>, DomainError> {
        let row = sqlx::query_as!(
            EmailVerificationCodeRow,
            r#"
            SELECT id, user_id, code_hash, expires_at, attempts, last_sent_at, created_at, magic_token_hash, magic_token_expires_at
            FROM email_verification_codes
            WHERE magic_token_hash = $1
            "#,
            magic_token_hash
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(row.map(Self::row_to_entity))
    }
}
