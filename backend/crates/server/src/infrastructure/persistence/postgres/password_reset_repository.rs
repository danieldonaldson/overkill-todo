//! `PostgreSQL` implementation of the password reset repository.

use async_trait::async_trait;
use std::sync::Arc;

use super::time::{ToDb, ToDomain};
use crate::application::ports::PasswordResetRepository;
use crate::domain::entities::PasswordResetCode;
use crate::domain::entities::attempt_lockout::{
    AttemptOutcome, IssueOutcome, LOCK_WINDOWS_MINUTES, LOCKOUT_MEMORY_DAYS, MAX_ATTEMPTS,
    RESEND_COOLDOWN_MINUTES,
};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::{ResetCodeId, UserId};
use crate::infrastructure::persistence::postgres::connection::Database;
use crate::infrastructure::persistence::postgres::error::persist_err;
use crate::infrastructure::persistence::postgres::models::PasswordResetCodeRow;

pub struct PostgresPasswordResetRepository {
    db: Arc<Database>,
}

impl PostgresPasswordResetRepository {
    #[must_use]
    pub const fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn row_to_entity(row: PasswordResetCodeRow) -> PasswordResetCode {
        PasswordResetCode::from_persistence(
            ResetCodeId::from_uuid(row.id),
            UserId::from_uuid(row.user_id),
            row.code_hash,
            row.expires_at.to_domain(),
            row.attempts,
            row.last_sent_at.to_domain(),
            row.created_at.to_domain(),
        )
        .with_lockouts(row.lockouts)
    }
}

#[async_trait]
impl PasswordResetRepository for PostgresPasswordResetRepository {
    #[tracing::instrument(name = "repo.password_reset.save", skip_all, fields(db.system = "postgresql", user_id = %code.user_id()))]
    async fn save(&self, code: &PasswordResetCode) -> Result<IssueOutcome, DomainError> {
        let stored = sqlx::query!(
            r#"
            INSERT INTO password_reset_codes (id, user_id, code_hash, expires_at, attempts, last_sent_at, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (user_id) DO UPDATE SET
                id = EXCLUDED.id,
                code_hash = EXCLUDED.code_hash,
                expires_at = EXCLUDED.expires_at,
                attempts = EXCLUDED.attempts,
                last_sent_at = EXCLUDED.last_sent_at,
                -- Replacing a code that sat at the attempt cap counts as a
                -- lockout and doubles the next lock window. Never reset here.
                lockouts = password_reset_codes.lockouts
                    + CASE WHEN password_reset_codes.attempts >= $8 THEN 1 ELSE 0 END
            -- P-11: the caller decided to reissue on a row it read earlier. A
            -- wrong guess may have locked that row since, or a concurrent
            -- request issued a code, so re-check the reissue rule here, in the
            -- statement that writes, at the new code's send time: a row at the
            -- cap is replaced only once its lock window (the domain's table,
            -- indexed by lockouts) has passed, and any row only once the
            -- resend cooldown has.
            WHERE (password_reset_codes.attempts < $8
                OR EXCLUDED.last_sent_at >= password_reset_codes.last_sent_at
                    + ($9::INT8[])[LEAST(GREATEST(password_reset_codes.lockouts, 0), cardinality($9::INT8[]) - 1) + 1]
                    * INTERVAL '1 minute')
              AND EXCLUDED.last_sent_at >= password_reset_codes.last_sent_at + $10::INT8 * INTERVAL '1 minute'
            RETURNING id
            "#,
            code.id().as_uuid(),
            code.user_id().as_uuid(),
            code.code_hash(),
            code.expires_at().to_db(),
            code.attempts(),
            code.last_sent_at().to_db(),
            code.created_at().to_db(),
            MAX_ATTEMPTS,
            &LOCK_WINDOWS_MINUTES[..],
            RESEND_COOLDOWN_MINUTES,
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(if stored.is_some() {
            IssueOutcome::Issued
        } else {
            IssueOutcome::Withheld
        })
    }

    #[tracing::instrument(name = "repo.password_reset.find_by_user", skip_all, fields(db.system = "postgresql", user_id = %user_id))]
    async fn find_by_user(
        &self,
        user_id: &UserId,
    ) -> Result<Option<PasswordResetCode>, DomainError> {
        let row = sqlx::query_as!(
            PasswordResetCodeRow,
            r#"
            SELECT id, user_id, code_hash, expires_at, attempts, last_sent_at, created_at, lockouts
            FROM password_reset_codes
            WHERE user_id = $1
            "#,
            user_id.as_uuid()
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(row.map(Self::row_to_entity))
    }

    #[tracing::instrument(name = "repo.password_reset.consume", skip_all, fields(db.system = "postgresql", user_id = %user_id, code_id = %code_id))]
    async fn consume(&self, user_id: &UserId, code_id: &ResetCodeId) -> Result<bool, DomainError> {
        let result = sqlx::query!(
            r#"
            DELETE FROM password_reset_codes
            WHERE user_id = $1 AND id = $2
            "#,
            user_id.as_uuid(),
            code_id.as_uuid(),
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(result.rows_affected() > 0)
    }

    #[tracing::instrument(name = "repo.password_reset.register_attempt", skip_all, fields(db.system = "postgresql", user_id = %user_id))]
    async fn register_attempt(
        &self,
        user_id: &UserId,
        code_id: &ResetCodeId,
        max_attempts: i32,
    ) -> Result<AttemptOutcome, DomainError> {
        // One atomic statement: the CTE claims a slot while below the cap, and
        // the outer SELECT reports whether a row exists at all, so callers can
        // tell "locked" (row present, at cap) from "no row" (deleted/expired
        // concurrently) without a second round trip or a misleading message.
        let row = sqlx::query!(
            r#"
            WITH bumped AS (
                UPDATE password_reset_codes
                SET attempts = attempts + 1
                WHERE user_id = $1 AND id = $3 AND attempts < $2
                RETURNING attempts
            )
            SELECT
                (SELECT attempts FROM bumped) AS "bumped_attempts?: i32",
                EXISTS (SELECT 1 FROM password_reset_codes WHERE user_id = $1 AND id = $3) AS "row_exists!"
            "#,
            user_id.as_uuid(),
            max_attempts,
            code_id.as_uuid(),
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

    #[tracing::instrument(name = "repo.password_reset.release_attempt", skip_all, fields(db.system = "postgresql", user_id = %user_id))]
    async fn release_attempt(
        &self,
        user_id: &UserId,
        code_id: &ResetCodeId,
    ) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            UPDATE password_reset_codes
            SET attempts = GREATEST(attempts - 1, 0)
            WHERE user_id = $1 AND id = $2
            "#,
            user_id.as_uuid(),
            code_id.as_uuid(),
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.password_reset.delete_expired", skip_all, fields(db.system = "postgresql"))]
    async fn delete_expired(&self) -> Result<u64, DomainError> {
        let result = sqlx::query!(
            r#"
            DELETE FROM password_reset_codes
            WHERE expires_at < NOW()
              AND (lockouts = 0 OR last_sent_at < NOW() - make_interval(days => $1))
            "#,
            LOCKOUT_MEMORY_DAYS,
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(result.rows_affected())
    }
}
