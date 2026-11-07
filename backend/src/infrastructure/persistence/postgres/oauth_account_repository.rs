//! `PostgreSQL` implementation of the OAuth account repository.

use async_trait::async_trait;
use std::sync::Arc;

use crate::application::ports::OAuthAccountRepository;
use crate::domain::entities::{OAuthAccount, OAuthProvider};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;
use crate::infrastructure::persistence::postgres::connection::Database;
use crate::infrastructure::persistence::postgres::error::persist_err;
use crate::infrastructure::persistence::postgres::models::OAuthAccountRow;

impl From<OAuthAccountRow> for OAuthAccount {
    fn from(row: OAuthAccountRow) -> Self {
        Self::from_persistence(
            row.id,
            UserId::from_uuid(row.user_id),
            row.provider,
            row.provider_user_id,
            row.provider_email,
            row.linked_at,
        )
    }
}

pub struct PostgresOAuthAccountRepository {
    db: Arc<Database>,
}

impl PostgresOAuthAccountRepository {
    #[must_use]
    pub const fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl OAuthAccountRepository for PostgresOAuthAccountRepository {
    #[tracing::instrument(name = "repo.oauth.find_by_provider_user_id", skip(self))]
    async fn find_by_provider_user_id(
        &self,
        provider: OAuthProvider,
        provider_user_id: &str,
    ) -> Result<Option<OAuthAccount>, DomainError> {
        let row = sqlx::query_as!(
            OAuthAccountRow,
            r#"
            SELECT id, user_id, provider as "provider: _", provider_user_id, provider_email, linked_at
            FROM user_oauth_accounts
            WHERE provider = $1 AND provider_user_id = $2
            "#,
            provider as OAuthProvider,
            provider_user_id,
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(row.map(OAuthAccount::from))
    }

    #[tracing::instrument(name = "repo.oauth.find_by_user_id", skip(self))]
    async fn find_by_user_id(&self, user_id: &UserId) -> Result<Vec<OAuthAccount>, DomainError> {
        let rows = sqlx::query_as!(
            OAuthAccountRow,
            r#"
            SELECT id, user_id, provider as "provider: _", provider_user_id, provider_email, linked_at
            FROM user_oauth_accounts
            WHERE user_id = $1
            ORDER BY linked_at ASC
            "#,
            user_id.as_uuid(),
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(rows.into_iter().map(OAuthAccount::from).collect())
    }

    #[tracing::instrument(name = "repo.oauth.save", skip(self, account))]
    async fn save(&self, account: &OAuthAccount) -> Result<(), DomainError> {
        sqlx::query!(
            r#"
            INSERT INTO user_oauth_accounts (id, user_id, provider, provider_user_id, provider_email, linked_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
            account.id(),
            account.user_id().as_uuid(),
            account.provider() as _,
            account.provider_user_id(),
            account.provider_email(),
            account.linked_at(),
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(())
    }

    #[tracing::instrument(name = "repo.oauth.delete_by_user_and_provider", skip(self))]
    async fn delete_by_user_and_provider(
        &self,
        user_id: &UserId,
        provider: OAuthProvider,
    ) -> Result<bool, DomainError> {
        let result = sqlx::query!(
            r#"
            DELETE FROM user_oauth_accounts
            WHERE user_id = $1 AND provider = $2
            "#,
            user_id.as_uuid(),
            provider as OAuthProvider,
        )
        .execute(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(result.rows_affected() > 0)
    }

    #[tracing::instrument(name = "repo.oauth.count_by_user_id", skip(self))]
    async fn count_by_user_id(&self, user_id: &UserId) -> Result<i64, DomainError> {
        let count = sqlx::query_scalar!(
            r#"
            SELECT COUNT(*) FROM user_oauth_accounts WHERE user_id = $1
            "#,
            user_id.as_uuid(),
        )
        .fetch_one(self.db.pool())
        .await
        .map_err(persist_err)?;

        Ok(count.unwrap_or(0))
    }
}
