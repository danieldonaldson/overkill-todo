//! OAuth account entity for linking external provider accounts to users.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::domain::value_objects::UserId;

/// Supported OAuth providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type)]
#[sqlx(type_name = "oauth_provider", rename_all = "snake_case")]
pub enum OAuthProvider {
    Google,
    Apple,
}

impl OAuthProvider {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Apple => "apple",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "google" => Some(Self::Google),
            "apple" => Some(Self::Apple),
            _ => None,
        }
    }
}

/// An OAuth account linked to a user.
///
/// Represents a connection between a user in our system and their account
/// with an external OAuth provider (Google, Apple).
#[derive(Debug, Clone)]
pub struct OAuthAccount {
    id: Uuid,
    user_id: UserId,
    provider: OAuthProvider,
    provider_user_id: String,
    provider_email: Option<String>,
    linked_at: DateTime<Utc>,
}

impl OAuthAccount {
    /// Creates a new OAuth account link.
    #[must_use]
    pub fn new(
        user_id: UserId,
        provider: OAuthProvider,
        provider_user_id: String,
        provider_email: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            user_id,
            provider,
            provider_user_id,
            provider_email,
            linked_at: Utc::now(),
        }
    }

    /// Reconstructs from persistence.
    #[must_use]
    pub const fn from_persistence(
        id: Uuid,
        user_id: UserId,
        provider: OAuthProvider,
        provider_user_id: String,
        provider_email: Option<String>,
        linked_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            user_id,
            provider,
            provider_user_id,
            provider_email,
            linked_at,
        }
    }

    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.id
    }

    #[must_use]
    pub const fn user_id(&self) -> &UserId {
        &self.user_id
    }

    #[must_use]
    pub const fn provider(&self) -> OAuthProvider {
        self.provider
    }

    #[must_use]
    pub fn provider_user_id(&self) -> &str {
        &self.provider_user_id
    }

    #[must_use]
    pub fn provider_email(&self) -> Option<&str> {
        self.provider_email.as_deref()
    }

    #[must_use]
    pub const fn linked_at(&self) -> DateTime<Utc> {
        self.linked_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_provider_as_str() {
        assert_eq!(OAuthProvider::Google.as_str(), "google");
        assert_eq!(OAuthProvider::Apple.as_str(), "apple");
    }

    #[test]
    fn test_oauth_provider_parse() {
        assert_eq!(OAuthProvider::parse("google"), Some(OAuthProvider::Google));
        assert_eq!(OAuthProvider::parse("apple"), Some(OAuthProvider::Apple));
        assert_eq!(OAuthProvider::parse("unknown"), None);
    }

    #[test]
    fn test_new_oauth_account() {
        let user_id = UserId::new();
        let account = OAuthAccount::new(
            user_id,
            OAuthProvider::Google,
            "google-sub-123".to_string(),
            Some("user@gmail.com".to_string()),
        );

        assert_eq!(account.user_id(), &user_id);
        assert_eq!(account.provider(), OAuthProvider::Google);
        assert_eq!(account.provider_user_id(), "google-sub-123");
        assert_eq!(account.provider_email(), Some("user@gmail.com"));
        assert!(!account.id().is_nil());
    }

    #[test]
    fn test_new_oauth_account_without_email() {
        let account = OAuthAccount::new(
            UserId::new(),
            OAuthProvider::Apple,
            "apple-sub-456".to_string(),
            None,
        );

        assert_eq!(account.provider(), OAuthProvider::Apple);
        assert_eq!(account.provider_user_id(), "apple-sub-456");
        assert_eq!(account.provider_email(), None);
    }

    #[test]
    fn test_from_persistence_preserves_all_fields() {
        let id = Uuid::now_v7();
        let user_id = UserId::new();
        let linked_at = Utc::now() - chrono::Duration::days(3);
        let account = OAuthAccount::from_persistence(
            id,
            user_id,
            OAuthProvider::Google,
            "sub-789".to_string(),
            Some("db@example.com".to_string()),
            linked_at,
        );

        assert_eq!(account.id(), id);
        assert_eq!(account.user_id(), &user_id);
        assert_eq!(account.provider(), OAuthProvider::Google);
        assert_eq!(account.provider_user_id(), "sub-789");
        assert_eq!(account.provider_email(), Some("db@example.com"));
        assert_eq!(account.linked_at(), linked_at);
    }
}
