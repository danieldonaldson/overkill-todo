//! Verification code generation port.

/// Trait for generating verification codes.
///
/// Implementations can generate random codes (production) or
/// fixed codes for specific test emails (E2E testing).
#[cfg_attr(any(test, feature = "mocks"), mockall::automock)]
pub trait CodeGenerator: Send + Sync {
    /// Generates a 6-digit verification code.
    ///
    /// # Arguments
    ///
    /// * `email` - The email address (used to check for test patterns)
    ///
    /// # Returns
    ///
    /// A 6-digit numeric code as a string (e.g., "123456")
    fn generate_code(&self, email: &str) -> String;
}
