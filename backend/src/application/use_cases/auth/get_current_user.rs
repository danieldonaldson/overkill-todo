//! Use case for retrieving the current authenticated user.

use crate::application::ports::UserRepository;
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;
use std::sync::Arc;

/// Use case for retrieving the current authenticated user.
pub struct GetCurrentUserUseCase {
    user_repository: Arc<dyn UserRepository>,
}

impl GetCurrentUserUseCase {
    /// Creates a new instance with the given repository.
    pub fn new(user_repository: Arc<dyn UserRepository>) -> Self {
        Self { user_repository }
    }

    /// Retrieves the user by their ID.
    ///
    /// Returns `None` if no user exists with the given ID.
    ///
    /// # Errors
    /// Returns an error if the repository fails to retrieve the user.
    #[tracing::instrument(name = "usecase.get_current_user", skip(self))]
    pub async fn execute(&self, user_id: &UserId) -> Result<Option<User>, DomainError> {
        let user = self.user_repository.find_by_id(user_id).await?;
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::value_objects::Email;

    #[tokio::test]
    async fn test_get_current_user_found() {
        let existing_user = User::new(
            Email::new("test@example.com").unwrap(),
            Some("hashed_password".to_string()),
            "John".to_string(),
            "Doe".to_string(),
        );
        let user_id = *existing_user.id();

        let mut mock_repo = MockUserRepository::new();
        let user_clone = existing_user.clone();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user_clone.clone())));

        let use_case = GetCurrentUserUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute(&user_id).await;

        assert!(result.is_ok());
        let user = result.unwrap();
        assert!(user.is_some());
        assert_eq!(user.unwrap().email().as_str(), "test@example.com");
    }

    #[tokio::test]
    async fn test_get_current_user_not_found() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(|_| Ok(None));

        let use_case = GetCurrentUserUseCase::new(Arc::new(mock_repo));
        let user_id = UserId::new();
        let result = use_case.execute(&user_id).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_get_current_user_repository_error() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(|_| Err(DomainError::Persistence("Database error".into())));

        let use_case = GetCurrentUserUseCase::new(Arc::new(mock_repo));
        let user_id = UserId::new();
        let result = use_case.execute(&user_id).await;

        assert!(result.is_err());
    }
}
