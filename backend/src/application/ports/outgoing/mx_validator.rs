use crate::domain::errors::DomainError;
use async_trait::async_trait;

/// Service trait for validating email domains via MX record lookup.
///
/// This trait defines the contract for checking if an email domain
/// has valid MX records (i.e., can receive email).
///
/// # Purpose
///
/// Validating MX records before sending emails helps:
/// - Reject obviously invalid email addresses (e.g., typos like gmail.con)
/// - Protect sender reputation by avoiding bounces
/// - Provide immediate feedback to users about typos
///
/// # Implementors
///
/// - [`crate::infrastructure::services::HickoryMxValidator`]
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait MxValidator: Send + Sync {
    /// Checks if a domain has valid MX records.
    ///
    /// # Arguments
    ///
    /// * `domain` - The email domain to check (e.g., "gmail.com")
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if the domain has at least one MX record
    /// - `Ok(false)` if the domain has no MX records
    /// - `Err(_)` if there was an error performing the DNS lookup
    async fn has_mx_record(&self, domain: &str) -> Result<bool, DomainError>;
}
