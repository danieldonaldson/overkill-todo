//! OAuth provider client port (driven port).
//!
//! Provider-agnostic interface for the OAuth authorization-code dance: build the
//! redirect URL, then exchange the returned code for the authenticated user's
//! profile. Adding a provider (Apple, GitHub, …) means implementing this trait
//! and registering it in the handler's provider map — no handler control flow
//! changes. Mirrors the ports used for every other external service.

use async_trait::async_trait;

use crate::application::dto::OAuthUserInfo;
use crate::domain::errors::DomainError;

/// A single OAuth provider's client (e.g. Google).
#[async_trait]
pub trait OAuthProviderClient: Send + Sync {
    /// Builds the provider authorization URL to redirect the user to, carrying
    /// the opaque `state` value for CSRF/session binding.
    fn authorization_url(&self, state: &str) -> String;

    /// Exchanges an authorization `code` for the authenticated user's info.
    ///
    /// # Errors
    /// Returns a [`DomainError`] if the token exchange or identity verification
    /// against the provider fails.
    async fn exchange_code(&self, code: &str) -> Result<OAuthUserInfo, DomainError>;
}
