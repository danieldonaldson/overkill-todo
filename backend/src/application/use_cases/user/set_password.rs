//! Use case for setting a password for OAuth-only users.
//!
//! This allows users who signed up via OAuth (and don't have a password)
//! to set one for the first time, enabling email/password login.

use crate::application::ports::{PasswordHasher, UserRepository};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;
use std::sync::Arc;

/// Use case for setting a password on an OAuth-only account.
///
/// Unlike [`ChangePasswordUseCase`], this doesn't require a current password
/// because the user authenticated via OAuth to reach this point.
pub struct SetPasswordUseCase {
    user_repository: Arc<dyn UserRepository>,
    password_hasher: Arc<dyn PasswordHasher>,
}

impl SetPasswordUseCase {
    /// Creates a new instance with the given repository and password hasher.
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        password_hasher: Arc<dyn PasswordHasher>,
    ) -> Self {
        Self {
            user_repository,
            password_hasher,
        }
    }

    /// Sets a password for an OAuth-only user.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The ID of the user setting their password
    /// * `new_password` - The password to set
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The user doesn't exist
    /// - The user already has a password (should use change password instead)
    #[tracing::instrument(name = "usecase.set_password", skip(self, new_password), fields(user_id = %user_id))]
    pub async fn execute(&self, user_id: &UserId, new_password: &str) -> Result<(), DomainError> {
        // Find the user
        let mut user = self
            .user_repository
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| DomainError::NotFound("User not found".into()))?;

        // Verify user doesn't already have a password
        if user.has_password() {
            return Err(DomainError::Validation(
                "User already has a password. Use 'Change Password' instead.".into(),
            ));
        }

        // Hash new password via the injected hasher port.
        let password_hash = self.password_hasher.hash(new_password).await?;

        // Update user
        user.update_password(password_hash);
        self.user_repository.save(&user).await?;

        tracing::info!(user_id = %user_id, "Password set for OAuth-only user");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::password_hasher::MockPasswordHasher;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::User;
    use crate::domain::errors::DomainError;
    use crate::domain::value_objects::Email;
    use proptest::prelude::*;

    fn create_oauth_only_user() -> User {
        User::new(
            Email::new("oauth@example.com").unwrap(),
            None,
            "OAuth".to_string(),
            "User".to_string(),
        )
    }

    /// A password hasher that returns a fixed hash without invoking Argon2.
    fn stub_hasher() -> MockPasswordHasher {
        let mut hasher = MockPasswordHasher::new();
        hasher
            .expect_hash()
            .returning(|_| Ok("$argon2id$stub".to_string()));
        hasher
    }

    fn create_user_with_password() -> User {
        User::new(
            Email::new("test@example.com").unwrap(),
            Some("$argon2id$existing".to_string()),
            "Test".to_string(),
            "User".to_string(),
        )
    }

    #[tokio::test]
    async fn test_set_password_success() {
        let user = create_oauth_only_user();
        let user_id = *user.id();
        let user_clone = user.clone();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user_clone.clone())));
        mock_repo.expect_save().times(1).returning(|_| Ok(()));

        let use_case = SetPasswordUseCase::new(Arc::new(mock_repo), Arc::new(stub_hasher()));
        let result = use_case.execute(&user_id, "new_secure_password").await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_set_password_user_not_found() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(|_| Ok(None));

        let use_case = SetPasswordUseCase::new(Arc::new(mock_repo), Arc::new(stub_hasher()));
        let result = use_case.execute(&UserId::new(), "new_password").await;

        assert!(matches!(result, Err(DomainError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_set_password_user_already_has_password() {
        let user = create_user_with_password();
        let user_id = *user.id();
        let user_clone = user.clone();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user_clone.clone())));

        let use_case = SetPasswordUseCase::new(Arc::new(mock_repo), Arc::new(stub_hasher()));
        let result = use_case.execute(&user_id, "new_password").await;

        assert!(matches!(result, Err(DomainError::Validation(_))));
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("already has a password"));
    }

    #[tokio::test]
    async fn test_set_password_repository_error() {
        let user = create_oauth_only_user();
        let user_id = *user.id();
        let user_clone = user.clone();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user_clone.clone())));
        mock_repo
            .expect_save()
            .times(1)
            .returning(|_| Err(DomainError::Persistence("Database error".into())));

        let use_case = SetPasswordUseCase::new(Arc::new(mock_repo), Arc::new(stub_hasher()));
        let result = use_case.execute(&user_id, "new_password").await;

        assert!(result.is_err());
    }

    // Property tests for set password logic
    // Tests the has_password check without invoking Argon2

    /// Create a user with a fake password hash (no Argon2, fast)
    fn create_user_with_password_fast() -> User {
        User::new(
            Email::new("test@example.com").unwrap(),
            Some("fake_hash_for_testing".to_string()), // No Argon2!
            "Test".to_string(),
            "User".to_string(),
        )
    }

    proptest! {
        /// OAuth-only users (no password) should be allowed to set a password
        /// This tests the invariant: !has_password() allows the operation
        #[test]
        fn prop_oauth_user_has_no_password(_seed in 0u64..1000) {
            let user = create_oauth_only_user();
            prop_assert!(!user.has_password());
            prop_assert!(user.password_hash().is_none());
        }

        /// Users with a password should be blocked from using set_password
        /// This tests the invariant: has_password() blocks the operation
        #[test]
        fn prop_user_with_password_is_blocked(_seed in 0u64..1000) {
            let user = create_user_with_password_fast();
            prop_assert!(user.has_password());
            prop_assert!(user.password_hash().is_some());
        }

        /// The has_password check is equivalent to password_hash.is_some()
        #[test]
        fn prop_has_password_matches_password_hash_some(_seed in 0u64..1000) {
            let oauth_user = create_oauth_only_user();
            let password_user = create_user_with_password_fast();

            prop_assert_eq!(oauth_user.has_password(), oauth_user.password_hash().is_some());
            prop_assert_eq!(password_user.has_password(), password_user.password_hash().is_some());
        }
    }
}
