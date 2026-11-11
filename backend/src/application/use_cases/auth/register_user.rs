//! Use case for the email-only first step of registration.

use crate::application::ports::{
    EmailService, MxValidator, OAuthAccountRepository, UserRepository,
};
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::Email;
use std::sync::Arc;

/// What registration found for the submitted email.
///
/// Callers must answer both variants identically: telling them apart in the
/// response would let anyone check whether an email has an account.
#[derive(Debug)]
pub enum RegisterOutcome {
    /// A new or still-incomplete signup: send it a verification code.
    PendingVerification(User),
    /// A completed account already uses this email: notify its owner with
    /// [`RegisterUserUseCase::notify_existing`] instead of sending a code.
    AlreadyRegistered(User),
}

pub struct RegisterUserUseCase {
    user_repository: Arc<dyn UserRepository>,
    oauth_account_repository: Arc<dyn OAuthAccountRepository>,
    email_service: Arc<dyn EmailService>,
    mx_validator: Arc<dyn MxValidator>,
}

impl RegisterUserUseCase {
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        oauth_account_repository: Arc<dyn OAuthAccountRepository>,
        email_service: Arc<dyn EmailService>,
        mx_validator: Arc<dyn MxValidator>,
    ) -> Self {
        Self {
            user_repository,
            oauth_account_repository,
            email_service,
            mx_validator,
        }
    }

    /// Logs a warning when the email's domain has no MX records.
    ///
    /// Advisory only: registration goes ahead either way. A typo'd domain just
    /// never receives the code, and a DNS failure shouldn't block signups.
    async fn warn_if_domain_lacks_mx(&self, email: &Email) {
        let domain = email.as_str().rsplit_once('@').map_or("", |(_, d)| d);
        match self.mx_validator.has_mx_record(domain).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(email_domain = %domain, "MX record check failed - domain may not receive emails");
            }
            Err(e) => {
                tracing::warn!(email_domain = %domain, error = %e, "MX validation error");
            }
        }
    }

    /// Creates a password-less user for `email`, or finds the existing one.
    ///
    /// # Errors
    /// Returns [`DomainError::Validation`] for a malformed email, or a
    /// persistence error from the repository.
    #[tracing::instrument(name = "usecase.register_user", skip_all)]
    pub async fn execute(&self, email: &str) -> Result<RegisterOutcome, DomainError> {
        let email_vo = Email::new(email)?;
        self.warn_if_domain_lacks_mx(&email_vo).await;

        if let Some(existing_user) = self.user_repository.find_by_email(email).await? {
            if existing_user.has_password() && existing_user.email_verified() {
                tracing::info!(
                    user_id = %existing_user.id(),
                    "Registration attempted for an existing account"
                );
                return Ok(RegisterOutcome::AlreadyRegistered(existing_user));
            }

            // INVARIANT: an unverified row is about to be handed to whoever
            // proves ownership of this email. Nobody has proven it yet, so no
            // OAuth identity may stay linked to it: a link here would let that
            // provider account log into the account the real owner completes.
            // (OAuth no longer creates such links; this clears any made before.)
            if !existing_user.email_verified() {
                let removed = self
                    .oauth_account_repository
                    .delete_all_for_user(existing_user.id())
                    .await?;
                if removed > 0 {
                    tracing::warn!(
                        user_id = %existing_user.id(),
                        removed,
                        "Removed OAuth links from an unverified signup being reused"
                    );
                }
            }

            tracing::info!(
                user_id = %existing_user.id(),
                "Reusing existing incomplete signup"
            );

            return Ok(RegisterOutcome::PendingVerification(existing_user));
        }

        let user = User::new(
            email_vo,
            None, // No password yet - set after email verification
            String::new(),
            String::new(),
        );

        self.user_repository.save(&user).await?;

        tracing::info!(user_id = %user.id(), "New user created for signup");

        Ok(RegisterOutcome::PendingVerification(user))
    }

    /// Emails the owner of an existing account that someone tried to register
    /// with their address.
    ///
    /// # Errors
    /// Returns the email service's error if the notice could not be sent.
    #[tracing::instrument(name = "usecase.register_user.notify_existing", skip_all, fields(user_id = %user.id()))]
    pub async fn notify_existing(&self, user: &User) -> Result<(), DomainError> {
        self.email_service
            .notify_account_exists(user.email().as_str(), user.first_name())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::email_service::MockEmailService;
    use crate::application::ports::outgoing::mx_validator::MockMxValidator;
    use crate::application::ports::outgoing::oauth_account_repository::MockOAuthAccountRepository;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::UserParts;
    use crate::domain::value_objects::AccountStatus;
    use crate::domain::value_objects::{UserId, UserRole};
    use chrono::Utc;

    fn create_incomplete_user() -> User {
        User::from_persistence(UserParts {
            id: UserId::new(),
            email: Email::new("test@example.com").unwrap(),
            password_hash: None, // No password
            first_name: String::new(),
            last_name: String::new(),
            email_verified: false, // Not verified
            pending_email_change: None,
            status: AccountStatus::Active,
            role: UserRole::User,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            version: 0,
        })
    }

    fn create_complete_user() -> User {
        User::from_persistence(UserParts {
            id: UserId::new(),
            email: Email::new("test@example.com").unwrap(),
            password_hash: Some("password_hash".to_string()), // Has password
            first_name: "John".to_string(),
            last_name: "Doe".to_string(),
            email_verified: true, // Verified
            pending_email_change: None,
            status: AccountStatus::Active,
            role: UserRole::User,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            version: 0,
        })
    }

    /// An MX validator that finds records for every domain.
    fn mx_ok() -> Arc<MockMxValidator> {
        let mut mx = MockMxValidator::new();
        mx.expect_has_mx_record().returning(|_| Ok(true));
        Arc::new(mx)
    }

    /// An OAuth repository that must not be touched.
    fn no_oauth_writes() -> MockOAuthAccountRepository {
        let mut oauth = MockOAuthAccountRepository::new();
        oauth.expect_delete_all_for_user().never();
        oauth
    }

    #[tokio::test]
    async fn test_register_new_user_success() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(|_| Ok(None));
        mock_repo.expect_save().times(1).returning(|_| Ok(()));

        let use_case = RegisterUserUseCase::new(
            Arc::new(mock_repo),
            Arc::new(no_oauth_writes()),
            Arc::new(MockEmailService::new()),
            mx_ok(),
        );
        let result = use_case.execute("test@example.com").await;

        let Ok(RegisterOutcome::PendingVerification(user)) = result else {
            panic!("expected a pending signup, got {result:?}");
        };
        assert_eq!(user.email().as_str(), "test@example.com");
        assert!(!user.has_password()); // No password yet
        assert!(!user.email_verified()); // Not verified yet
    }

    #[tokio::test]
    async fn test_register_reuses_incomplete_signup() {
        let incomplete_user = create_incomplete_user();
        let user_id = *incomplete_user.id();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(move |_| Ok(Some(incomplete_user.clone())));
        // Note: save() should NOT be called - we reuse existing user
        let mut oauth = MockOAuthAccountRepository::new();
        oauth
            .expect_delete_all_for_user()
            .withf(move |id| *id == user_id)
            .times(1)
            .returning(|_| Ok(0));

        let use_case = RegisterUserUseCase::new(
            Arc::new(mock_repo),
            Arc::new(oauth),
            Arc::new(MockEmailService::new()),
            mx_ok(),
        );
        let result = use_case.execute("test@example.com").await;

        let Ok(RegisterOutcome::PendingVerification(user)) = result else {
            panic!("expected a pending signup, got {result:?}");
        };
        assert_eq!(user.id(), &user_id); // Same user returned
    }

    /// A verified, password-less account (an OAuth-only user) keeps its links:
    /// its owner already proved the email, and anyone can type it into the form.
    #[tokio::test]
    async fn test_register_keeps_oauth_links_of_verified_passwordless_user() {
        let mut oauth_only = create_incomplete_user();
        oauth_only.mark_email_verified();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(move |_| Ok(Some(oauth_only.clone())));

        let use_case = RegisterUserUseCase::new(
            Arc::new(mock_repo),
            Arc::new(no_oauth_writes()),
            Arc::new(MockEmailService::new()),
            mx_ok(),
        );
        let result = use_case.execute("test@example.com").await;

        assert!(matches!(
            result,
            Ok(RegisterOutcome::PendingVerification(_))
        ));
    }

    #[tokio::test]
    async fn test_register_reports_complete_user_as_already_registered() {
        let complete_user = create_complete_user();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(move |_| Ok(Some(complete_user.clone())));

        let use_case = RegisterUserUseCase::new(
            Arc::new(mock_repo),
            Arc::new(no_oauth_writes()),
            Arc::new(MockEmailService::new()),
            mx_ok(),
        );
        let result = use_case.execute("test@example.com").await;

        // Not an error: the handler must answer exactly as for a new signup.
        assert!(matches!(result, Ok(RegisterOutcome::AlreadyRegistered(_))));
    }

    #[tokio::test]
    async fn notify_existing_sends_the_account_exists_email() {
        let user = create_complete_user();
        let mut email = MockEmailService::new();
        email
            .expect_notify_account_exists()
            .withf(|to, _| to == "test@example.com")
            .times(1)
            .returning(|_, _| Ok(()));

        let use_case = RegisterUserUseCase::new(
            Arc::new(MockUserRepository::new()),
            Arc::new(no_oauth_writes()),
            Arc::new(email),
            mx_ok(),
        );
        use_case.notify_existing(&user).await.unwrap();
    }

    #[tokio::test]
    async fn test_register_invalid_email() {
        let mock_repo = MockUserRepository::new();

        let use_case = RegisterUserUseCase::new(
            Arc::new(mock_repo),
            Arc::new(no_oauth_writes()),
            Arc::new(MockEmailService::new()),
            mx_ok(),
        );
        let result = use_case.execute("invalid-email").await;

        assert!(result.is_err());
        assert!(matches!(result, Err(DomainError::Validation(_))));
    }

    #[tokio::test]
    async fn test_register_repository_error() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(|_| Ok(None));
        mock_repo
            .expect_save()
            .times(1)
            .returning(|_| Err(DomainError::Persistence("Database error".into())));

        let use_case = RegisterUserUseCase::new(
            Arc::new(mock_repo),
            Arc::new(no_oauth_writes()),
            Arc::new(MockEmailService::new()),
            mx_ok(),
        );
        let result = use_case.execute("test@example.com").await;

        assert!(result.is_err());
    }
}
