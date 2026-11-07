//! Use case for refreshing authentication tokens.

use crate::application::ports::UserRepository;
use crate::application::ports::{RefreshTokenRepository, TokenService};
use crate::domain::entities::RefreshToken;
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use chrono::{Duration, Utc};
use std::sync::Arc;

/// Result of a successful token refresh operation.
#[derive(Debug)]
pub struct RefreshResult {
    /// The authenticated user.
    pub user: User,
    /// The new refresh token (plaintext, to be sent to client).
    pub new_refresh_token: String,
}

/// Use case for refreshing authentication tokens.
///
/// Validates the provided refresh token and device fingerprint,
/// then issues a new refresh token (rotation) while revoking the old one.
pub struct RefreshTokenUseCase {
    refresh_token_repository: Arc<dyn RefreshTokenRepository>,
    user_repository: Arc<dyn UserRepository>,
    token_service: Arc<dyn TokenService>,
}

impl RefreshTokenUseCase {
    /// Creates a new instance with the given dependencies.
    pub fn new(
        refresh_token_repository: Arc<dyn RefreshTokenRepository>,
        user_repository: Arc<dyn UserRepository>,
        token_service: Arc<dyn TokenService>,
    ) -> Self {
        Self {
            refresh_token_repository,
            user_repository,
            token_service,
        }
    }

    /// Refreshes an authentication token.
    ///
    /// # Arguments
    ///
    /// * `token` - The current refresh token (plaintext)
    /// * `device_fingerprint` - The device fingerprint hash from the client
    ///
    /// # Returns
    ///
    /// A [`RefreshResult`] containing the user and new refresh token on success.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::Authorization`] if:
    /// - The token is invalid, expired, or revoked
    /// - The device fingerprint doesn't match
    /// - The user no longer exists
    #[tracing::instrument(name = "usecase.refresh_token", skip(self, token, device_fingerprint))]
    pub async fn execute(
        &self,
        token: &str,
        device_fingerprint: &str,
    ) -> Result<RefreshResult, DomainError> {
        // Hash the provided token to look it up
        let token_hash = self.token_service.hash_token(token);

        // Find the token in the database
        let refresh_token = self
            .refresh_token_repository
            .find_by_hash(&token_hash)
            .await?
            .ok_or_else(|| DomainError::Authorization("Invalid refresh token".into()))?;

        // Check if token is valid (not expired, not revoked)
        if !refresh_token.is_valid() {
            // INVARIANT: test the revoked-reuse condition BEFORE expiry. A
            // stolen refresh token replayed after its natural expiry is both
            // revoked and expired; checking expiry first would skip full-family
            // revocation and let the theft go undetected.
            if refresh_token.is_revoked() {
                // Revoked token presented again — likely stolen; invalidate all sessions.
                tracing::warn!(
                    user_id = %refresh_token.user_id(),
                    "Revoked refresh token reuse detected — revoking all sessions for user"
                );
                self.refresh_token_repository
                    .revoke_all_for_user(refresh_token.user_id())
                    .await?;
                return Err(DomainError::Authorization(
                    "Refresh token has been revoked".into(),
                ));
            }
            return Err(DomainError::Authorization(
                "Refresh token has expired".into(),
            ));
        }

        // Validate device fingerprint
        if !refresh_token.validate_fingerprint(device_fingerprint) {
            // Log this as a potential security issue
            tracing::warn!(
                user_id = %refresh_token.user_id(),
                "Device fingerprint mismatch during token refresh - possible token theft"
            );
            return Err(DomainError::Authorization(
                "Device fingerprint mismatch".into(),
            ));
        }

        // Get the user
        let user = self
            .user_repository
            .find_by_id(refresh_token.user_id())
            .await?
            .ok_or_else(|| DomainError::Authorization("User not found".into()))?;

        // Atomically revoke the old token and save the new one
        let new_token = self.token_service.generate_token();
        let new_hash = self.token_service.hash_token(&new_token);
        let expiration_days = self.token_service.refresh_token_expiration_days();
        let new_expires = Utc::now() + Duration::days(expiration_days);

        let new_refresh_token = RefreshToken::new(
            *user.id(),
            new_hash,
            device_fingerprint.to_string(),
            new_expires,
        );

        self.refresh_token_repository
            .rotate(refresh_token.id(), &new_refresh_token)
            .await?;

        tracing::info!(user_id = %user.id(), "Token refreshed successfully");

        Ok(RefreshResult {
            user,
            new_refresh_token: new_token,
        })
    }

    /// Creates a new refresh token for a user (used after login/signup).
    ///
    /// # Errors
    ///
    /// Returns an error if token creation or repository operations fail.
    pub async fn create_for_user(
        &self,
        user_id: &crate::domain::value_objects::UserId,
        device_fingerprint: &str,
    ) -> Result<String, DomainError> {
        let token_plaintext = self.token_service.generate_token();
        let token_hash = self.token_service.hash_token(&token_plaintext);
        let expires_at =
            Utc::now() + Duration::days(self.token_service.refresh_token_expiration_days());

        let refresh_token = RefreshToken::new(
            *user_id,
            token_hash,
            device_fingerprint.to_string(),
            expires_at,
        );

        self.refresh_token_repository.save(&refresh_token).await?;

        Ok(token_plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::refresh_token_repository::MockRefreshTokenRepository;
    use crate::application::ports::outgoing::token_service::MockTokenService;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::UserBuilder;
    use crate::domain::value_objects::{RefreshTokenId, UserId};

    const FINGERPRINT: &str = "device-fingerprint";

    fn test_user(id: UserId) -> User {
        UserBuilder::new()
            .id(id)
            .email("refresh@example.com")
            .password_hash(Some("hash"))
            .build()
    }

    /// A live (not expired, not revoked) refresh token bound to `FINGERPRINT`.
    fn valid_token(user_id: UserId) -> RefreshToken {
        RefreshToken::new(
            user_id,
            "stored-hash".to_string(),
            FINGERPRINT.to_string(),
            Utc::now() + Duration::days(30),
        )
    }

    /// A token that is both revoked AND naturally expired.
    fn revoked_and_expired_token(user_id: UserId) -> RefreshToken {
        RefreshToken::from_persistence(
            RefreshTokenId::new(),
            user_id,
            "stored-hash".to_string(),
            FINGERPRINT.to_string(),
            Utc::now() - Duration::days(1),
            Some(Utc::now() - Duration::hours(1)),
            Utc::now() - Duration::days(31),
        )
    }

    fn expired_token(user_id: UserId) -> RefreshToken {
        RefreshToken::from_persistence(
            RefreshTokenId::new(),
            user_id,
            "stored-hash".to_string(),
            FINGERPRINT.to_string(),
            Utc::now() - Duration::hours(1),
            None,
            Utc::now() - Duration::days(31),
        )
    }

    fn token_service() -> MockTokenService {
        let mut svc = MockTokenService::new();
        svc.expect_hash_token().returning(|t| format!("hashed:{t}"));
        svc.expect_generate_token()
            .returning(|| "new-plaintext-token".to_string());
        svc.expect_refresh_token_expiration_days().returning(|| 30);
        svc
    }

    #[tokio::test]
    async fn rotates_token_on_valid_refresh() {
        let user_id = UserId::new();
        let token = valid_token(user_id);
        let old_id = *token.id();

        let mut repo = MockRefreshTokenRepository::new();
        repo.expect_find_by_hash()
            .times(1)
            .returning(move |_| Ok(Some(token.clone())));
        repo.expect_rotate()
            .times(1)
            .withf(move |id, new| id == &old_id && new.device_fingerprint() == FINGERPRINT)
            .returning(|_, _| Ok(()));
        repo.expect_revoke_all_for_user().never();

        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(test_user(user_id))));

        let uc = RefreshTokenUseCase::new(
            Arc::new(repo),
            Arc::new(user_repo),
            Arc::new(token_service()),
        );

        let result = uc.execute("plaintext", FINGERPRINT).await.unwrap();
        assert_eq!(result.new_refresh_token, "new-plaintext-token");
        assert_eq!(result.user.id(), &user_id);
    }

    #[tokio::test]
    async fn rejects_unknown_token() {
        let mut repo = MockRefreshTokenRepository::new();
        repo.expect_find_by_hash().times(1).returning(|_| Ok(None));
        repo.expect_revoke_all_for_user().never();

        let uc = RefreshTokenUseCase::new(
            Arc::new(repo),
            Arc::new(MockUserRepository::new()),
            Arc::new(token_service()),
        );

        let err = uc.execute("plaintext", FINGERPRINT).await.unwrap_err();
        assert!(matches!(err, DomainError::Authorization(_)));
    }

    /// A revoked token that is *also* expired must still trigger
    /// full-family revocation — the revoked check runs before the expiry check.
    #[tokio::test]
    async fn revoked_reuse_revokes_all_even_when_also_expired() {
        let user_id = UserId::new();
        let token = revoked_and_expired_token(user_id);

        let mut repo = MockRefreshTokenRepository::new();
        repo.expect_find_by_hash()
            .times(1)
            .returning(move |_| Ok(Some(token.clone())));
        repo.expect_revoke_all_for_user()
            .times(1)
            .withf(move |uid| uid == &user_id)
            .returning(|_| Ok(3));
        repo.expect_rotate().never();

        let uc = RefreshTokenUseCase::new(
            Arc::new(repo),
            Arc::new(MockUserRepository::new()),
            Arc::new(token_service()),
        );

        let err = uc.execute("plaintext", FINGERPRINT).await.unwrap_err();
        assert!(matches!(err, DomainError::Authorization(_)));
    }

    /// An expired-but-never-revoked token is a normal expiry: reject without
    /// nuking the user's other sessions.
    #[tokio::test]
    async fn expired_token_does_not_revoke_all() {
        let user_id = UserId::new();
        let token = expired_token(user_id);

        let mut repo = MockRefreshTokenRepository::new();
        repo.expect_find_by_hash()
            .times(1)
            .returning(move |_| Ok(Some(token.clone())));
        repo.expect_revoke_all_for_user().never();
        repo.expect_rotate().never();

        let uc = RefreshTokenUseCase::new(
            Arc::new(repo),
            Arc::new(MockUserRepository::new()),
            Arc::new(token_service()),
        );

        let err = uc.execute("plaintext", FINGERPRINT).await.unwrap_err();
        assert!(matches!(err, DomainError::Authorization(_)));
    }

    #[tokio::test]
    async fn rejects_device_fingerprint_mismatch() {
        let user_id = UserId::new();
        let token = valid_token(user_id);

        let mut repo = MockRefreshTokenRepository::new();
        repo.expect_find_by_hash()
            .times(1)
            .returning(move |_| Ok(Some(token.clone())));
        repo.expect_rotate().never();
        repo.expect_revoke_all_for_user().never();

        let uc = RefreshTokenUseCase::new(
            Arc::new(repo),
            Arc::new(MockUserRepository::new()),
            Arc::new(token_service()),
        );

        let err = uc.execute("plaintext", "wrong-device").await.unwrap_err();
        assert!(matches!(err, DomainError::Authorization(_)));
    }

    #[tokio::test]
    async fn rejects_when_user_no_longer_exists() {
        let user_id = UserId::new();
        let token = valid_token(user_id);

        let mut repo = MockRefreshTokenRepository::new();
        repo.expect_find_by_hash()
            .times(1)
            .returning(move |_| Ok(Some(token.clone())));
        repo.expect_rotate().never();

        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_id()
            .times(1)
            .returning(|_| Ok(None));

        let uc = RefreshTokenUseCase::new(
            Arc::new(repo),
            Arc::new(user_repo),
            Arc::new(token_service()),
        );

        let err = uc.execute("plaintext", FINGERPRINT).await.unwrap_err();
        assert!(matches!(err, DomainError::Authorization(_)));
    }

    /// The concurrency loser: `rotate` returns `Conflict` (the row was already
    /// rotated by a peer). The error propagates rather than minting a token.
    #[tokio::test]
    async fn propagates_rotate_conflict() {
        let user_id = UserId::new();
        let token = valid_token(user_id);

        let mut repo = MockRefreshTokenRepository::new();
        repo.expect_find_by_hash()
            .times(1)
            .returning(move |_| Ok(Some(token.clone())));
        repo.expect_rotate()
            .times(1)
            .returning(|_, _| Err(DomainError::Conflict("already rotated".into())));

        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(test_user(user_id))));

        let uc = RefreshTokenUseCase::new(
            Arc::new(repo),
            Arc::new(user_repo),
            Arc::new(token_service()),
        );

        let err = uc.execute("plaintext", FINGERPRINT).await.unwrap_err();
        assert!(matches!(err, DomainError::Conflict(_)));
    }

    /// `create_for_user` mints a fresh token bound to the given user + device,
    /// persists it (hashed), and returns the plaintext to the caller.
    #[tokio::test]
    async fn create_for_user_issues_and_persists_token() {
        let user_id = UserId::new();

        let mut repo = MockRefreshTokenRepository::new();
        repo.expect_save()
            .times(1)
            .withf(move |rt| {
                rt.user_id() == &user_id
                    && rt.device_fingerprint() == FINGERPRINT
                    // generate_token() → "new-plaintext-token", hashed by the
                    // mock token service to "hashed:new-plaintext-token".
                    && rt.token_hash() == "hashed:new-plaintext-token"
            })
            .returning(|_| Ok(()));
        repo.expect_rotate().never();

        let uc = RefreshTokenUseCase::new(
            Arc::new(repo),
            Arc::new(MockUserRepository::new()),
            Arc::new(token_service()),
        );

        let token = uc.create_for_user(&user_id, FINGERPRINT).await.unwrap();
        assert_eq!(token, "new-plaintext-token");
    }

    /// A persistence failure while saving the new token propagates.
    #[tokio::test]
    async fn create_for_user_propagates_save_error() {
        let user_id = UserId::new();

        let mut repo = MockRefreshTokenRepository::new();
        repo.expect_save()
            .times(1)
            .returning(|_| Err(DomainError::Persistence("save boom".into())));

        let uc = RefreshTokenUseCase::new(
            Arc::new(repo),
            Arc::new(MockUserRepository::new()),
            Arc::new(token_service()),
        );

        let err = uc.create_for_user(&user_id, FINGERPRINT).await.unwrap_err();
        assert!(matches!(err, DomainError::Persistence(_)));
    }
}
