//! Turnstile (CAPTCHA) service port.

use crate::domain::errors::DomainError;
use async_trait::async_trait;

/// Trait for CAPTCHA verification using Cloudflare Turnstile.
#[cfg_attr(any(test, feature = "mocks"), mockall::automock)]
#[async_trait]
pub trait TurnstileService: Send + Sync {
    /// Verifies a Turnstile token received from the frontend.
    ///
    /// # Arguments
    ///
    /// * `token` - The Turnstile response token from the frontend widget
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if the token is valid
    /// - `Ok(false)` if the token is invalid (bot detected)
    /// - `Err(_)` if there was an error communicating with Cloudflare
    async fn verify(&self, token: &str) -> Result<bool, DomainError>;

    /// Returns `true` if bot protection is active.
    ///
    /// When this returns `true`, callers MUST require a Turnstile token and
    /// reject requests that omit one — otherwise the check is bypassable by
    /// simply not sending the token. Implementations that always pass (e.g.
    /// the no-op used in local development) return `false`.
    fn is_enabled(&self) -> bool;
}
