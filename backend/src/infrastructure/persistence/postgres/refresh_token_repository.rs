//! `PostgreSQL` implementation of the refresh token repository.

use async_trait::async_trait;
use std::sync::Arc;

use crate::application::ports::RefreshTokenRepository;
use crate::domain::entities::RefreshToken;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::{RefreshTokenId, UserId};
use crate::infrastructure::persistence::postgres::connection::Database;
use crate::infrastructure::persistence::postgres::error::persist_err;
use crate::infrastructure::persistence::postgres::models::RefreshTokenRow;

pub struct PostgresRefreshTokenRepository {
    db: Arc<Database>,
}

impl PostgresRefreshTokenRepository {
    #[must_use]
    pub const fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn row_to_entity(row: RefreshTokenRow) -> RefreshToken {
        RefreshToken::from_persistence(
            RefreshTokenId::from_uuid(row.id),
            UserId::from_uuid(row.user_id),
            row.token_hash,
            row.device_fingerprint,
            row.expires_at,
            row.revoked_at,
            row.created_at,
        )
    }
}

#[async_trait]
impl RefreshTokenRepository for PostgresRefreshTokenRepository {
    #[tracing::instrument(name = "repo.refresh_token.save", skip(self, token), fields(user_id = %token.user_id()))]
    async fn save(&self, token: &RefreshToken) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            INSERT INTO refresh_tokens (id, user_id, token_hash, device_fingerprint, expires_at, revoked_at, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
            token.id().as_uuid(),
            token.user_id().as_uuid(),
            token.token_hash(),
            token.device_fingerprint(),
            token.expires_at(),
            token.revoked_at(),
            token.created_at(),
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.refresh_token.find_by_hash", skip(self, token_hash))]
    async fn find_by_hash(&self, token_hash: &str) -> Result<Option<RefreshToken>, DomainError> {
        let row = sqlx::query_as!(
            RefreshTokenRow,
            r#"
            SELECT id, user_id, token_hash, device_fingerprint, expires_at, revoked_at, created_at
            FROM refresh_tokens
            WHERE token_hash = $1
            "#,
            token_hash
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(row.map(Self::row_to_entity))
    }

    #[tracing::instrument(name = "repo.refresh_token.revoke", skip(self), fields(token_id = %id))]
    async fn revoke(&self, id: &RefreshTokenId) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            UPDATE refresh_tokens
            SET revoked_at = NOW()
            WHERE id = $1 AND revoked_at IS NULL
            "#,
            id.as_uuid()
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.refresh_token.revoke_all_for_user", skip(self), fields(user_id = %user_id))]
    async fn revoke_all_for_user(&self, user_id: &UserId) -> Result<u64, DomainError> {
        let result = sqlx::query!(
            r#"
            UPDATE refresh_tokens
            SET revoked_at = NOW()
            WHERE user_id = $1 AND revoked_at IS NULL
            "#,
            user_id.as_uuid()
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(result.rows_affected())
    }

    #[tracing::instrument(name = "repo.refresh_token.revoke_all_for_user_except", skip(self, keep_token_hash), fields(user_id = %user_id))]
    async fn revoke_all_for_user_except(
        &self,
        user_id: &UserId,
        keep_token_hash: &str,
    ) -> Result<u64, DomainError> {
        let result = sqlx::query!(
            r#"
            UPDATE refresh_tokens
            SET revoked_at = NOW()
            WHERE user_id = $1 AND revoked_at IS NULL AND token_hash <> $2
            "#,
            user_id.as_uuid(),
            keep_token_hash
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(result.rows_affected())
    }

    #[tracing::instrument(name = "repo.refresh_token.rotate", skip(self, new_token))]
    async fn rotate(
        &self,
        old_id: &RefreshTokenId,
        new_token: &RefreshToken,
    ) -> Result<(), DomainError> {
        let mut tx = self.db.pool().begin().await.map_err(persist_err)?;

        let revoked = sqlx::query!(
            r#"
            UPDATE refresh_tokens
            SET revoked_at = NOW()
            WHERE id = $1 AND revoked_at IS NULL
            "#,
            old_id.as_uuid()
        )
        .execute(&mut *tx)
        .await
        .map_err(persist_err)?;

        // Under concurrency the row is locked by the first transaction; the loser
        // re-evaluates the predicate after commit and matches zero rows. Aborting
        // here ensures a token is rotated exactly once and never mints two successors.
        if revoked.rows_affected() == 0 {
            return Err(DomainError::Conflict(
                "Refresh token has already been rotated".into(),
            ));
        }

        sqlx::query!(
            r#"
            INSERT INTO refresh_tokens (id, user_id, token_hash, device_fingerprint, expires_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
            new_token.id().as_uuid(),
            new_token.user_id().as_uuid(),
            new_token.token_hash(),
            new_token.device_fingerprint(),
            new_token.expires_at(),
        )
        .execute(&mut *tx)
        .await
        .map_err(persist_err)?;

        tx.commit().await.map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.refresh_token.delete_expired", skip(self))]
    async fn delete_expired(&self) -> Result<u64, DomainError> {
        let result = sqlx::query!(
            r#"
            DELETE FROM refresh_tokens
            WHERE expires_at < NOW()
            "#,
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(result.rows_affected())
    }
}
