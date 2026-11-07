use std::fmt;

use crate::domain::errors::DomainError;

/// A validated email address.
///
/// Email addresses are normalized on creation:
/// - Trimmed of leading/trailing whitespace
/// - Converted to lowercase
///
/// # Validation
///
/// Must contain both `@` and `.` characters.
///
/// # Examples
///
/// ```
/// use app::domain::value_objects::Email;
///
/// // Valid email, normalized to lowercase
/// let email = Email::new("  User@Example.COM  ").unwrap();
/// assert_eq!(email.as_str(), "user@example.com");
///
/// // Invalid email (missing @)
/// assert!(Email::new("invalid").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email(String);

impl Email {
    /// Creates a new validated email address.
    ///
    /// The input is trimmed and lowercased before validation.
    ///
    /// # Errors
    ///
    /// Returns [`DomainError::Validation`] if:
    /// - The email is empty after trimming
    /// - The email doesn't contain `@` or `.`
    pub fn new(value: &str) -> Result<Self, DomainError> {
        let value = value.trim().to_lowercase();

        if value.is_empty() {
            return Err(DomainError::Validation("Email cannot be empty".into()));
        }

        if !value.contains('@') || !value.contains('.') {
            return Err(DomainError::Validation("Invalid email format".into()));
        }

        Ok(Self(value))
    }

    /// Returns the email address as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the email and returns the inner string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

/// Allows conversion from [`String`] to [`Email`].
///
/// # Examples
///
/// ```
/// use app::domain::value_objects::Email;
///
/// let email: Email = "user@example.com".to_string().try_into().unwrap();
/// ```
impl TryFrom<String> for Email {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl fmt::Display for Email {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_valid_email() {
        let result = Email::new("user@example.com");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "user@example.com");
    }

    #[test]
    fn test_email_trimmed_and_lowercased() {
        let result = Email::new("  USER@Example.COM  ");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "user@example.com");
    }

    #[test]
    fn test_empty_email() {
        let result = Email::new("");
        assert!(result.is_err());
        match result {
            Err(DomainError::Validation(msg)) => {
                assert!(msg.contains("cannot be empty"));
            }
            _ => panic!("Expected Validation error"),
        }
    }

    #[test]
    fn test_email_without_at() {
        let result = Email::new("userexample.com");
        assert!(result.is_err());
        match result {
            Err(DomainError::Validation(msg)) => {
                assert!(msg.contains("Invalid email format"));
            }
            _ => panic!("Expected Validation error"),
        }
    }

    #[test]
    fn test_email_without_dot() {
        let result = Email::new("user@examplecom");
        assert!(result.is_err());
        match result {
            Err(DomainError::Validation(msg)) => {
                assert!(msg.contains("Invalid email format"));
            }
            _ => panic!("Expected Validation error"),
        }
    }

    #[test]
    fn test_whitespace_only_email_is_empty_error() {
        let result = Email::new("    ");
        match result {
            Err(DomainError::Validation(msg)) => {
                assert!(msg.contains("cannot be empty"));
            }
            _ => panic!("Expected Validation error"),
        }
    }

    #[test]
    fn test_into_string_returns_normalized_value() {
        let email = Email::new("  User@Example.COM  ").unwrap();
        assert_eq!(email.into_string(), "user@example.com");
    }

    #[test]
    fn test_try_from_string_valid() {
        let email: Email = "user@example.com".to_string().try_into().unwrap();
        assert_eq!(email.as_str(), "user@example.com");
    }

    #[test]
    fn test_try_from_string_invalid() {
        let result: Result<Email, _> = "invalid".to_string().try_into();
        assert!(matches!(result, Err(DomainError::Validation(_))));
    }

    #[test]
    fn test_display_matches_as_str() {
        let email = Email::new("user@example.com").unwrap();
        assert_eq!(format!("{email}"), "user@example.com");
        assert_eq!(email.to_string(), email.as_str());
    }

    // ==================== Property-Based Tests ====================

    proptest! {
        // Valid email format: local@domain.tld
        #[test]
        fn prop_valid_email_format_accepted(
            local in "[a-z][a-z0-9]{0,10}",
            domain in "[a-z]{2,8}",
            tld in "[a-z]{2,4}",
        ) {
            let email_str = format!("{local}@{domain}.{tld}");
            let result = Email::new(&email_str);
            prop_assert!(result.is_ok(), "Valid email {} rejected", email_str);
        }

        // Normalization: emails are lowercased
        #[test]
        fn prop_email_normalized_to_lowercase(
            input in "[A-Za-z]{1,8}@[A-Za-z]{2,6}\\.[A-Za-z]{2,4}",
        ) {
            let result = Email::new(&input);
            prop_assert!(result.is_ok());
            let email = result.unwrap();
            prop_assert_eq!(email.as_str(), input.to_lowercase());
        }

        // Whitespace is trimmed
        #[test]
        fn prop_email_whitespace_trimmed(
            spaces_before in " {0,3}",
            email in "[a-z]{1,6}@[a-z]{2,6}\\.[a-z]{2,3}",
            spaces_after in " {0,3}",
        ) {
            let input = format!("{spaces_before}{email}{spaces_after}");
            let result = Email::new(&input);
            prop_assert!(result.is_ok());
            let parsed = result.unwrap();
            prop_assert_eq!(parsed.as_str(), email);
        }

        // Missing @ is rejected
        #[test]
        fn prop_missing_at_rejected(input in "[a-z]{3,15}\\.[a-z]{2,4}") {
            let result = Email::new(&input);
            prop_assert!(result.is_err());
        }

        // Missing . is rejected
        #[test]
        fn prop_missing_dot_rejected(input in "[a-z]{1,8}@[a-z]{3,12}") {
            let result = Email::new(&input);
            prop_assert!(result.is_err());
        }

        // Only whitespace is rejected
        #[test]
        fn prop_whitespace_only_rejected(spaces in " {0,10}") {
            let result = Email::new(&spaces);
            prop_assert!(result.is_err());
        }
    }
}
