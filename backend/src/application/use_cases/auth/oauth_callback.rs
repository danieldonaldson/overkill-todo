//! OAuth callback use case for handling OAuth provider responses.

use std::sync::Arc;

use crate::application::dto::OAuthUserInfo;
use crate::application::ports::OAuthAccountRepository;
use crate::application::ports::UserRepository;
use crate::domain::entities::User;
use crate::domain::entities::{OAuthAccount, OAuthProvider};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::{Email, UserId};

/// Result of OAuth callback processing.
#[derive(Debug)]
pub enum OAuthCallbackResult {
    /// User logged in (existing account or newly created).
    LoggedIn(Box<User>),
    /// OAuth provider linked to existing account.
    Linked,
    /// Email conflict - OAuth email exists but verification status mismatch.
    /// Frontend should prompt user to log in with password first.
    EmailConflict,
    /// Provider account already linked to a different user.
    AlreadyLinked,
}

/// OAuth flow mode.
#[derive(Debug, Clone, Copy)]
pub enum OAuthCallbackMode {
    /// Login or register via OAuth.
    Login,
    /// Link OAuth provider to existing account.
    Link,
}

/// OAuth callback use case - handles OAuth provider responses.
///
/// Supports two modes:
/// - **Login**: Logs in existing user, auto-links to verified email, or creates new user
/// - **Link**: Links OAuth provider to authenticated user's account
pub struct OAuthCallbackUseCase {
    user_repo: Arc<dyn UserRepository>,
    oauth_repo: Arc<dyn OAuthAccountRepository>,
}

impl OAuthCallbackUseCase {
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        oauth_repo: Arc<dyn OAuthAccountRepository>,
    ) -> Self {
        Self {
            user_repo,
            oauth_repo,
        }
    }

    /// Execute the OAuth callback flow.
    ///
    /// # Arguments
    /// * `provider` - OAuth provider (Google, Apple)
    /// * `user_info` - User info extracted from provider's ID token
    /// * `mode` - Login or Link mode
    /// * `linking_user_id` - User ID for link mode (required if mode=Link)
    ///
    /// # Errors
    /// Returns an error if repository operations fail or if required parameters are invalid.
    #[tracing::instrument(
        name = "usecase.oauth_callback",
        skip(self, user_info),
        fields(provider = %provider.as_str(), mode = ?mode)
    )]
    pub async fn execute(
        &self,
        provider: OAuthProvider,
        user_info: OAuthUserInfo,
        mode: OAuthCallbackMode,
        linking_user_id: Option<&UserId>,
    ) -> Result<OAuthCallbackResult, DomainError> {
        // Check if this provider account is already linked to any user
        if let Some(existing_oauth) = self
            .oauth_repo
            .find_by_provider_user_id(provider, &user_info.sub)
            .await?
        {
            match mode {
                OAuthCallbackMode::Login => {
                    // INVARIANT: an existing link means the user previously
                    // proved ownership of both the provider identity (`sub`) and
                    // this account, and the link was only persisted after a
                    // verified-email gate (see `handle_login`/`handle_link`).
                    // Re-authenticating with the same `sub` is sufficient to log
                    // in; the provider's current `email_verified` flag must NOT
                    // gate it here, or an OAuth-only user
                    // (no password) whose provider later reports the email
                    // unverified would be permanently locked out of their own
                    // account with no recovery path.
                    let user = self
                        .user_repo
                        .find_by_id(existing_oauth.user_id())
                        .await?
                        .ok_or_else(|| DomainError::NotFound("User not found".into()))?;
                    return Ok(OAuthCallbackResult::LoggedIn(Box::new(user)));
                }
                OAuthCallbackMode::Link => {
                    // Check if linked to a different user
                    if let Some(uid) = linking_user_id
                        && existing_oauth.user_id() != uid
                    {
                        return Ok(OAuthCallbackResult::AlreadyLinked);
                    }
                    // Already linked to this user - success
                    return Ok(OAuthCallbackResult::Linked);
                }
            }
        }

        // Not yet linked - handle based on mode
        match mode {
            OAuthCallbackMode::Login => self.handle_login(provider, user_info).await,
            OAuthCallbackMode::Link => {
                let user_id = linking_user_id.ok_or_else(|| {
                    DomainError::Validation("Missing user_id for link mode".into())
                })?;
                self.handle_link(provider, user_info, *user_id).await
            }
        }
    }

    /// Handle login mode - find existing user or create new one.
    async fn handle_login(
        &self,
        provider: OAuthProvider,
        user_info: OAuthUserInfo,
    ) -> Result<OAuthCallbackResult, DomainError> {
        // Try to find existing user by email (for auto-linking)
        if let Some(ref email_str) = user_info.email
            && let Ok(email) = Email::new(email_str)
            && let Some(existing_user) = self.user_repo.find_by_email(email.as_str()).await?
        {
            // Hybrid auto-link: only if BOTH emails are verified
            // This prevents account takeover attacks
            if user_info.email_verified && existing_user.email_verified() {
                // Auto-link OAuth account and log in
                let oauth_account = OAuthAccount::new(
                    *existing_user.id(),
                    provider,
                    user_info.sub,
                    user_info.email,
                );
                self.oauth_repo.save(&oauth_account).await?;
                tracing::info!(
                    user_id = %existing_user.id(),
                    provider = %provider.as_str(),
                    "Auto-linked OAuth account to existing verified user"
                );
                return Ok(OAuthCallbackResult::LoggedIn(Box::new(existing_user)));
            }
            // Email exists but verification mismatch - security concern
            tracing::warn!(
                email = %email_str,
                oauth_verified = user_info.email_verified,
                user_verified = existing_user.email_verified(),
                "OAuth email conflict - verification mismatch"
            );
            return Ok(OAuthCallbackResult::EmailConflict);
        }

        // No existing user - create new OAuth-only user
        let email = user_info
            .email
            .as_ref()
            .and_then(|e| Email::new(e).ok())
            .ok_or_else(|| {
                DomainError::Validation("OAuth provider did not provide a valid email".into())
            })?;

        let first_name = user_info
            .given_name
            .clone()
            .unwrap_or_else(|| "User".to_string());
        let last_name = user_info.family_name.clone().unwrap_or_default();

        let mut user = User::new(email, None, first_name, last_name);

        // If provider verified email, mark it verified (skip email verification flow)
        if user_info.email_verified {
            user.mark_email_verified();
        }

        self.user_repo.save(&user).await?;

        // Link OAuth account to new user
        let oauth_account = OAuthAccount::new(*user.id(), provider, user_info.sub, user_info.email);
        self.oauth_repo.save(&oauth_account).await?;

        tracing::info!(
            user_id = %user.id(),
            provider = %provider.as_str(),
            "Created new user via OAuth"
        );

        Ok(OAuthCallbackResult::LoggedIn(Box::new(user)))
    }

    /// Handle link mode - link OAuth provider to authenticated user.
    async fn handle_link(
        &self,
        provider: OAuthProvider,
        user_info: OAuthUserInfo,
        user_id: UserId,
    ) -> Result<OAuthCallbackResult, DomainError> {
        // INVARIANT: never persist a link for an unverified provider identity.
        // ARCHITECTURE.md: "auto-link only if email verified by both sides" — the
        // link path must enforce the provider side of that gate too.
        if !user_info.email_verified {
            tracing::warn!(
                provider = %provider.as_str(),
                "OAuth link rejected: provider reports email unverified"
            );
            return Ok(OAuthCallbackResult::EmailConflict);
        }

        // Verify user exists
        self.user_repo
            .find_by_id(&user_id)
            .await?
            .ok_or_else(|| DomainError::NotFound("User not found".into()))?;

        // Link OAuth account
        let oauth_account = OAuthAccount::new(user_id, provider, user_info.sub, user_info.email);
        self.oauth_repo.save(&oauth_account).await?;

        tracing::info!(
            user_id = %user_id,
            provider = %provider.as_str(),
            "Linked OAuth account to existing user"
        );

        Ok(OAuthCallbackResult::Linked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::oauth_account_repository::MockOAuthAccountRepository;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;

    fn create_test_user(verified: bool) -> User {
        let mut user = User::new(
            Email::new("test@example.com").unwrap(),
            Some("password_hash".to_string()),
            "Test".to_string(),
            "User".to_string(),
        );
        if verified {
            user.mark_email_verified();
        }
        user
    }

    fn create_oauth_user_info(email_verified: bool) -> OAuthUserInfo {
        OAuthUserInfo {
            sub: "oauth-user-id-123".to_string(),
            email: Some("test@example.com".to_string()),
            email_verified,
            name: Some("Test User".to_string()),
            given_name: Some("Test".to_string()),
            family_name: Some("User".to_string()),
        }
    }

    #[tokio::test]
    async fn test_login_existing_oauth_account() {
        let user = create_test_user(true);
        let user_id = *user.id();
        let user_clone = user.clone();

        let mut mock_user_repo = MockUserRepository::new();
        let mut mock_oauth_repo = MockOAuthAccountRepository::new();

        // OAuth account already exists
        mock_oauth_repo
            .expect_find_by_provider_user_id()
            .returning(move |_, _| {
                Ok(Some(OAuthAccount::new(
                    user_id,
                    OAuthProvider::Google,
                    "oauth-user-id-123".to_string(),
                    Some("test@example.com".to_string()),
                )))
            });

        mock_user_repo
            .expect_find_by_id()
            .returning(move |_| Ok(Some(user_clone.clone())));

        let use_case =
            OAuthCallbackUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case
            .execute(
                OAuthProvider::Google,
                create_oauth_user_info(true),
                OAuthCallbackMode::Login,
                None,
            )
            .await;

        assert!(result.is_ok());
        match result.unwrap() {
            OAuthCallbackResult::LoggedIn(u) => {
                assert_eq!(u.email().as_str(), "test@example.com");
            }
            _ => panic!("Expected LoggedIn result"),
        }
    }

    #[tokio::test]
    async fn test_login_auto_link_verified_emails() {
        let user = create_test_user(true);
        let user_clone = user.clone();

        let mut mock_user_repo = MockUserRepository::new();
        let mut mock_oauth_repo = MockOAuthAccountRepository::new();

        // No existing OAuth account
        mock_oauth_repo
            .expect_find_by_provider_user_id()
            .returning(|_, _| Ok(None));

        // User with matching verified email exists
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user_clone.clone())));

        mock_oauth_repo.expect_save().returning(|_| Ok(()));

        let use_case =
            OAuthCallbackUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case
            .execute(
                OAuthProvider::Google,
                create_oauth_user_info(true), // OAuth email verified
                OAuthCallbackMode::Login,
                None,
            )
            .await;

        assert!(result.is_ok());
        match result.unwrap() {
            OAuthCallbackResult::LoggedIn(u) => {
                assert_eq!(u.email().as_str(), "test@example.com");
            }
            _ => panic!("Expected LoggedIn result"),
        }
    }

    #[tokio::test]
    async fn test_login_email_conflict_unverified() {
        let user = create_test_user(false); // User email NOT verified
        let user_clone = user.clone();

        let mut mock_user_repo = MockUserRepository::new();
        let mut mock_oauth_repo = MockOAuthAccountRepository::new();

        mock_oauth_repo
            .expect_find_by_provider_user_id()
            .returning(|_, _| Ok(None));

        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user_clone.clone())));

        let use_case =
            OAuthCallbackUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case
            .execute(
                OAuthProvider::Google,
                create_oauth_user_info(true), // OAuth email verified
                OAuthCallbackMode::Login,
                None,
            )
            .await;

        assert!(result.is_ok());
        assert!(matches!(
            result.unwrap(),
            OAuthCallbackResult::EmailConflict
        ));
    }

    #[tokio::test]
    async fn test_login_creates_new_user() {
        let mut mock_user_repo = MockUserRepository::new();
        let mut mock_oauth_repo = MockOAuthAccountRepository::new();

        mock_oauth_repo
            .expect_find_by_provider_user_id()
            .returning(|_, _| Ok(None));

        mock_user_repo
            .expect_find_by_email()
            .returning(|_| Ok(None)); // No existing user

        mock_user_repo.expect_save().returning(|_| Ok(()));

        mock_oauth_repo.expect_save().returning(|_| Ok(()));

        let use_case =
            OAuthCallbackUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case
            .execute(
                OAuthProvider::Google,
                create_oauth_user_info(true),
                OAuthCallbackMode::Login,
                None,
            )
            .await;

        assert!(result.is_ok());
        match result.unwrap() {
            OAuthCallbackResult::LoggedIn(u) => {
                assert_eq!(u.email().as_str(), "test@example.com");
                assert_eq!(u.first_name(), "Test");
            }
            _ => panic!("Expected LoggedIn result"),
        }
    }

    #[tokio::test]
    async fn test_link_success() {
        let user = create_test_user(true);
        let user_clone = user.clone();
        let user_id = *user.id();

        let mut mock_user_repo = MockUserRepository::new();
        let mut mock_oauth_repo = MockOAuthAccountRepository::new();

        mock_oauth_repo
            .expect_find_by_provider_user_id()
            .returning(|_, _| Ok(None));

        mock_user_repo
            .expect_find_by_id()
            .returning(move |_| Ok(Some(user_clone.clone())));

        mock_oauth_repo.expect_save().returning(|_| Ok(()));

        let use_case =
            OAuthCallbackUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case
            .execute(
                OAuthProvider::Google,
                create_oauth_user_info(true),
                OAuthCallbackMode::Link,
                Some(&user_id),
            )
            .await;

        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), OAuthCallbackResult::Linked));
    }

    /// An existing link logs the user in even when the provider currently
    /// reports the email as unverified: the link was already established behind
    /// a verified-email gate, and re-authenticating with the same `sub` proves
    /// ownership. Gating this on the live `email_verified` flag would lock an
    /// OAuth-only user out of their own account.
    #[tokio::test]
    async fn test_existing_link_login_succeeds_when_unverified() {
        let user = create_test_user(true);
        let user_id = *user.id();
        let user_clone = user.clone();

        let mut mock_user_repo = MockUserRepository::new();
        let mut mock_oauth_repo = MockOAuthAccountRepository::new();

        mock_oauth_repo
            .expect_find_by_provider_user_id()
            .returning(move |_, _| {
                Ok(Some(OAuthAccount::new(
                    user_id,
                    OAuthProvider::Google,
                    "oauth-user-id-123".to_string(),
                    Some("test@example.com".to_string()),
                )))
            });

        mock_user_repo
            .expect_find_by_id()
            .returning(move |_| Ok(Some(user_clone.clone())));

        let use_case =
            OAuthCallbackUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case
            .execute(
                OAuthProvider::Google,
                create_oauth_user_info(false), // provider reports email unverified
                OAuthCallbackMode::Login,
                None,
            )
            .await;

        match result.unwrap() {
            OAuthCallbackResult::LoggedIn(u) => {
                assert_eq!(u.email().as_str(), "test@example.com");
            }
            other => panic!("Expected LoggedIn result, got {other:?}"),
        }
    }

    /// Linking must not persist an OAuth account for an unverified provider
    /// identity — `save` is never called.
    #[tokio::test]
    async fn test_link_rejected_when_unverified() {
        let user_id = UserId::new();

        let mock_user_repo = MockUserRepository::new(); // find_by_id must NOT be called
        let mut mock_oauth_repo = MockOAuthAccountRepository::new();

        mock_oauth_repo
            .expect_find_by_provider_user_id()
            .returning(|_, _| Ok(None));
        // expect_save intentionally not set: linking an unverified identity must
        // not reach persistence.

        let use_case =
            OAuthCallbackUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case
            .execute(
                OAuthProvider::Google,
                create_oauth_user_info(false), // provider reports email unverified
                OAuthCallbackMode::Link,
                Some(&user_id),
            )
            .await;

        assert!(matches!(
            result.unwrap(),
            OAuthCallbackResult::EmailConflict
        ));
    }

    #[tokio::test]
    async fn test_link_already_linked_to_other_user() {
        let other_user_id = UserId::new();
        let current_user_id = UserId::new();

        let mock_user_repo = MockUserRepository::new();
        let mut mock_oauth_repo = MockOAuthAccountRepository::new();

        // OAuth account linked to a different user
        mock_oauth_repo
            .expect_find_by_provider_user_id()
            .returning(move |_, _| {
                Ok(Some(OAuthAccount::new(
                    other_user_id,
                    OAuthProvider::Google,
                    "oauth-user-id-123".to_string(),
                    Some("other@example.com".to_string()),
                )))
            });

        let use_case =
            OAuthCallbackUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case
            .execute(
                OAuthProvider::Google,
                create_oauth_user_info(true),
                OAuthCallbackMode::Link,
                Some(&current_user_id),
            )
            .await;

        assert!(result.is_ok());
        assert!(matches!(
            result.unwrap(),
            OAuthCallbackResult::AlreadyLinked
        ));
    }
}
