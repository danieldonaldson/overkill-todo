//! Use case for exporting a user's personal data.
//!
//! Assembles the personal data the system holds about a user — their profile and
//! linked OAuth accounts — for the GDPR data-portability right. Returns the
//! domain entities; the handler maps them to the machine-readable response DTO.

use std::sync::Arc;

use crate::ports::{AuditLogRepository, OAuthAccountRepository, UserRepository};
use domain::entities::{AuditEvent, OAuthAccount, User};
use domain::errors::DomainError;
use domain::value_objects::UserId;

/// Use case for exporting the authenticated user's personal data.
pub struct ExportUserDataUseCase {
    user_repository: Arc<dyn UserRepository>,
    oauth_account_repository: Arc<dyn OAuthAccountRepository>,
    audit_log: Arc<dyn AuditLogRepository>,
}

impl ExportUserDataUseCase {
    #[must_use]
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        oauth_account_repository: Arc<dyn OAuthAccountRepository>,
        audit_log: Arc<dyn AuditLogRepository>,
    ) -> Self {
        Self {
            user_repository,
            oauth_account_repository,
            audit_log,
        }
    }

    /// Gathers the user profile and their linked OAuth accounts.
    ///
    /// # Errors
    /// - `NotFound` if the user doesn't exist.
    /// - `Persistence` on a repository failure.
    #[tracing::instrument(name = "usecase.export_user_data", skip_all, fields(user_id = %user_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
    ) -> Result<(User, Vec<OAuthAccount>), DomainError> {
        let user = self
            .user_repository
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| DomainError::NotFound("User not found".into()))?;

        let oauth_accounts = self
            .oauth_account_repository
            .find_by_user_id(user_id)
            .await?;

        // Recorded before the data is handed over: if the event can't be
        // stored, the export fails rather than happening unrecorded (OT-008 D7).
        self.audit_log
            .record(&AuditEvent::data_exported(*user_id))
            .await?;

        tracing::info!(user_id = %user_id, oauth_count = oauth_accounts.len(), "User data exported");
        Ok((user, oauth_accounts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::outgoing::audit_log_repository::MockAuditLogRepository;
    use crate::ports::outgoing::oauth_account_repository::MockOAuthAccountRepository;
    use crate::ports::outgoing::user_repository::MockUserRepository;
    use domain::entities::oauth_account::OAuthProvider;
    use domain::entities::{User, UserBuilder};

    fn make_user() -> User {
        UserBuilder::new().password_hash(Some("hash")).build()
    }

    #[tokio::test]
    async fn exports_user_and_oauth_accounts() {
        let user = make_user();
        let user_id = *user.id();
        let account = OAuthAccount::new(
            user_id,
            OAuthProvider::Google,
            "sub-123".to_string(),
            Some("test@example.com".to_string()),
        );

        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user.clone())));

        let mut oauth_repo = MockOAuthAccountRepository::new();
        oauth_repo
            .expect_find_by_user_id()
            .times(1)
            .returning(move |_| Ok(vec![account.clone()]));

        let mut audit_log = MockAuditLogRepository::new();
        audit_log
            .expect_record()
            .withf(move |e| *e == AuditEvent::data_exported(user_id))
            .times(1)
            .returning(|_| Ok(()));

        let uc = ExportUserDataUseCase::new(
            Arc::new(user_repo),
            Arc::new(oauth_repo),
            Arc::new(audit_log),
        );
        let (exported_user, accounts) = uc.execute(&user_id).await.unwrap();
        assert_eq!(exported_user.email().as_str(), "test@example.com");
        assert_eq!(accounts.len(), 1);
    }

    #[tokio::test]
    async fn export_fails_when_its_event_cannot_be_recorded() {
        let user = make_user();
        let user_id = *user.id();
        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));
        let mut oauth_repo = MockOAuthAccountRepository::new();
        oauth_repo
            .expect_find_by_user_id()
            .returning(|_| Ok(Vec::new()));
        let mut audit_log = MockAuditLogRepository::new();
        audit_log
            .expect_record()
            .returning(|_| Err(DomainError::Persistence("audit table down".into())));

        let uc = ExportUserDataUseCase::new(
            Arc::new(user_repo),
            Arc::new(oauth_repo),
            Arc::new(audit_log),
        );
        let err = uc.execute(&user_id).await.unwrap_err();
        assert!(matches!(err, DomainError::Persistence(_)));
    }

    #[tokio::test]
    async fn missing_user_is_not_found() {
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_id().returning(|_| Ok(None));

        // Nothing exported, nothing recorded: the mock refuses any call.
        let uc = ExportUserDataUseCase::new(
            Arc::new(user_repo),
            Arc::new(MockOAuthAccountRepository::new()),
            Arc::new(MockAuditLogRepository::new()),
        );
        let err = uc.execute(&UserId::new()).await.unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)));
    }
}
