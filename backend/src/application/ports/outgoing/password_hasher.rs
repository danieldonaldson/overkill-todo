//! Password and code hashing service port.

use async_trait::async_trait;

use crate::domain::errors::DomainError;

/// Trait for hashing and verifying passwords/codes.
///
/// Uses Argon2 for secure hashing of passwords and 6-digit verification codes.
///
/// The methods are `async` because Argon2 is a deliberately expensive CPU-bound
/// operation; implementations are expected to offload the work to a blocking
/// thread (`tokio::task::spawn_blocking`) so a hash/verify can't stall an async
/// worker for the full configured cost.
#[cfg_attr(any(test, feature = "mocks"), mockall::automock)]
#[async_trait]
pub trait PasswordHasher: Send + Sync {
    /// Hashes a password or verification code.
    ///
    /// # Arguments
    ///
    /// * `password` - The plaintext password or code to hash
    ///
    /// # Returns
    ///
    /// The hashed password in PHC string format.
    ///
    /// # Errors
    ///
    /// Returns an error if hashing fails.
    async fn hash(&self, password: &str) -> Result<String, DomainError>;

    /// Verifies a password or code against its hash.
    ///
    /// # Arguments
    ///
    /// * `password` - The plaintext password or code to verify
    /// * `hash` - The stored hash to verify against
    ///
    /// # Returns
    ///
    /// `true` if the password matches, `false` otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error if verification fails.
    async fn verify(&self, password: &str, hash: &str) -> Result<bool, DomainError>;
}
