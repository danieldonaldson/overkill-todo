//! Domain-specific error types.

use thiserror::Error;

/// Errors that can occur in the domain layer.
///
/// These errors represent domain-level failures and are designed to be
/// mapped to appropriate HTTP status codes by the infrastructure layer.
///
/// # Mapping to HTTP Status
///
/// | Variant | HTTP Status |
/// |---------|-------------|
/// | [`DomainError::Validation`] | 400 Bad Request |
/// | [`DomainError::Authorization`] | 401 Unauthorized |
/// | [`DomainError::Forbidden`] | 403 Forbidden |
/// | [`DomainError::NotFound`] | 404 Not Found |
/// | [`DomainError::Conflict`] | 409 Conflict |
/// | [`DomainError::BusinessRule`] | 422 Unprocessable Entity |
/// | [`DomainError::Persistence`] | 500 Internal Server Error |
/// | [`DomainError::ExternalService`] | 502 Bad Gateway |
///
/// # Examples
///
/// ```
/// use app::domain::errors::DomainError;
///
/// fn validate_age(age: i32) -> Result<(), DomainError> {
///     if age < 0 {
///         return Err(DomainError::Validation("Age cannot be negative".into()));
///     }
///     Ok(())
/// }
///
/// assert!(validate_age(-1).is_err());
/// ```
#[derive(Error, Debug)]
pub enum DomainError {
    /// Input validation failed (e.g., invalid email format, negative values).
    #[error("Validation error: {0}")]
    Validation(String),

    /// Authentication failed (e.g., invalid credentials, expired token).
    #[error("Authorization error: {0}")]
    Authorization(String),

    /// Access forbidden (e.g., insufficient permissions, unverified email).
    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// Requested entity does not exist.
    #[error("Entity not found: {0}")]
    NotFound(String),

    /// Resource already exists or state conflict.
    #[error("Conflict: {0}")]
    Conflict(String),

    /// A business rule was violated (e.g., an account cannot be deleted twice).
    #[error("Business rule violation: {0}")]
    BusinessRule(String),

    /// Database or persistence layer error.
    #[error("Persistence error: {0}")]
    Persistence(String),

    /// External service error (e.g., SMTP, Turnstile, DNS lookup).
    #[error("External service error: {0}")]
    ExternalService(String),
}
