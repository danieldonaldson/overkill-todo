//! `PostgreSQL` implementation of the password reset repository.

use async_trait::async_trait;
use std::sync::Arc;

use crate::application::ports::PasswordResetRepository;
use crate::domain::entities::PasswordResetCode;
use crate::domain::entities::attempt_lockout::AttemptOutcome;
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
            row.expires_at,
            row.attempts,
            row.last_sent_at,
            row.created_at,
        )
    }
}

#[async_trait]
impl PasswordResetRepository for PostgresPasswordResetRepository {
    #[tracing::instrument(name = "repo.password_reset.save", skip(self, code), fields(user_id = %code.user_id()))]
    async fn save(&self, code: &PasswordResetCode) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            INSERT INTO password_reset_codes (id, user_id, code_hash, expires_at, attempts, last_sent_at, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (user_id) DO UPDATE SET
                id = EXCLUDED.id,
                code_hash = EXCLUDED.code_hash,
                expires_at = EXCLUDED.expires_at,
                attempts = EXCLUDED.attempts,
                last_sent_at = EXCLUDED.last_sent_at
            "#,
            code.id().as_uuid(),
            code.user_id().as_uuid(),
            code.code_hash(),
            code.expires_at(),
            code.attempts(),
            code.last_sent_at(),
            code.created_at(),
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.password_reset.find_by_user", skip(self), fields(user_id = %user_id))]
    async fn find_by_user(
        &self,
        user_id: &UserId,
    ) -> Result<Option<PasswordResetCode>, DomainError> {
        let row = sqlx::query_as!(
            PasswordResetCodeRow,
            r#"
            SELECT id, user_id, code_hash, expires_at, attempts, last_sent_at, created_at
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

    #[tracing::instrument(name = "repo.password_reset.consume", skip(self), fields(user_id = %user_id, code_id = %code_id))]
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

    #[tracing::instrument(name = "repo.password_reset.register_attempt", skip(self), fields(user_id = %user_id))]
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
                UPDATE password_reset_codes
                SET attempts = attempts + 1
                WHERE user_id = $1 AND attempts < $2
                RETURNING attempts
            )
            SELECT
                (SELECT attempts FROM bumped) AS "bumped_attempts?: i32",
                EXISTS (SELECT 1 FROM password_reset_codes WHERE user_id = $1) AS "row_exists!"
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

    #[tracing::instrument(name = "repo.password_reset.release_attempt", skip(self), fields(user_id = %user_id))]
    async fn release_attempt(&self, user_id: &UserId) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            UPDATE password_reset_codes
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

    #[tracing::instrument(name = "repo.password_reset.delete_expired", skip(self))]
    async fn delete_expired(&self) -> Result<u64, DomainError> {
        let result = sqlx::query!(
            r#"
            DELETE FROM password_reset_codes
            WHERE expires_at < NOW()
            "#,
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(result.rows_affected())
    }
}
