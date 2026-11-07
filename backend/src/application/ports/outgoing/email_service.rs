//! Email service port for sending transactional emails.

use crate::domain::errors::DomainError;
use async_trait::async_trait;

/// Trait for sending transactional emails.
///
/// Implementations handle the actual email delivery (e.g., via SMTP).
#[cfg_attr(any(test, feature = "mocks"), mockall::automock)]
#[async_trait]
pub trait EmailService: Send + Sync {
    /// Sends an email verification code to a user.
    ///
    /// # Arguments
    ///
    /// * `to` - The recipient's email address
    /// * `code` - The 6-digit verification code (plaintext)
    /// * `first_name` - The user's first name for personalization
    /// * `magic_link` - Optional one-click verification URL. When `Some`, the
    ///   email also offers a clickable link (in addition to the code); when
    ///   `None` (e.g. a code-only resend), only the code is sent.
    async fn send_verification_email(
        &self,
        to: &str,
        code: &str,
        first_name: &str,
        magic_link: Option<String>,
    ) -> Result<(), DomainError>;

    /// Sends a password reset code to a user.
    ///
    /// # Arguments
    ///
    /// * `to` - The recipient's email address
    /// * `code` - The 6-digit reset code (plaintext)
    /// * `first_name` - The user's first name for personalization
    async fn send_password_reset_email(
        &self,
        to: &str,
        code: &str,
        first_name: &str,
    ) -> Result<(), DomainError>;
}
