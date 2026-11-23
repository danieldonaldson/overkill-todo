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
/// use overkill_todo::domain::value_objects::Email;
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
        let value = normalize(value);

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

/// Trims surrounding whitespace and lowercases (threat model P-30).
///
/// ASCII input, which is nearly every address, takes a byte-wise path that
/// gives exactly what `trim().to_lowercase()` gives (a proptest checks this).
/// It's there so the Kani proofs finish: the solver can't get through Unicode
/// lowercasing (design D7).
fn normalize(value: &str) -> String {
    if !value.is_ascii() {
        return value.trim().to_lowercase();
    }
    let (start, end) = ascii_trim_range(value.as_bytes());
    value[start..end].to_ascii_lowercase()
}

/// The range `str::trim` keeps of an ASCII string. `str::trim` strips
/// `char::is_whitespace`, which for ASCII is tab, line feed, vertical tab,
/// form feed, carriage return and space (`u8::is_ascii_whitespace` leaves out
/// the vertical tab).
fn ascii_trim_range(bytes: &[u8]) -> (usize, usize) {
    let is_space = |b: &u8| matches!(b, b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b' ');
    let start = bytes
        .iter()
        .position(|b| !is_space(b))
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !is_space(b))
        .map_or(start, |i| i + 1);
    (start, end)
}

/// Allows conversion from [`String`] to [`Email`].
///
/// # Examples
///
/// ```
/// use overkill_todo::domain::value_objects::Email;
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

/// Kani proofs (OT-010, threat model P-30, design D7). Run with `just verify`.
///
/// `Email::new` validates the normalised string, so two inputs with the same
/// normalisation always get the same result; the proofs cover the
/// normalisation. Inputs are ASCII strings of up to 8 bytes, over the ASCII
/// path (`ascii_trim_range`, then lowercasing each byte, which is what
/// `str::to_ascii_lowercase` does). Unicode lowercasing and building an
/// `Email` are intractable for the solver; the proptests cover Unicode, and
/// one checks the ASCII path equals `trim().to_lowercase()`.
#[cfg(kani)]
mod verification {
    use super::*;

    const MAX_LEN: usize = 8;
    /// Room for an input plus one whitespace byte at each end.
    const CAP: usize = MAX_LEN + 2;

    /// Any ASCII string of up to 8 bytes, as bytes and a length.
    fn any_ascii() -> ([u8; CAP], usize) {
        let bytes: [u8; CAP] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= MAX_LEN);
        kani::assume(bytes.iter().all(u8::is_ascii));
        (bytes, len)
    }

    /// The ASCII normalisation of `input`, as bytes and a length.
    fn normalized(input: &[u8]) -> ([u8; CAP], usize) {
        let (start, end) = ascii_trim_range(input);
        let mut out = [0u8; CAP];
        for (slot, b) in out.iter_mut().zip(&input[start..end]) {
            *slot = b.to_ascii_lowercase();
        }
        (out, end - start)
    }

    fn same(a: ([u8; CAP], usize), b: ([u8; CAP], usize)) -> bool {
        a.1 == b.1 && a.0[..a.1] == b.0[..b.1]
    }

    /// For every ASCII input of up to 8 bytes: normalising the normalised
    /// form changes nothing.
    #[kani::proof]
    #[kani::unwind(12)]
    fn normalising_twice_changes_nothing() {
        let (bytes, len) = any_ascii();
        let once = normalized(&bytes[..len]);
        assert!(same(normalized(&once.0[..once.1]), once));
    }

    /// For every ASCII input of up to 8 bytes, and every variant of it with
    /// any letters' case flipped and any whitespace byte added at either end:
    /// both normalise to the same address.
    #[kani::proof]
    #[kani::unwind(12)]
    fn case_and_whitespace_variants_are_one_identity() {
        let (bytes, len) = any_ascii();
        let pad = || -> Option<u8> {
            let b: u8 = kani::any();
            kani::assume(matches!(b, 0 | b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b' '));
            (b != 0).then_some(b)
        };
        let mut variant = [0u8; CAP];
        let mut n = 0;
        if let Some(b) = pad() {
            variant[n] = b;
            n += 1;
        }
        for &b in &bytes[..len] {
            variant[n] = if kani::any() {
                b.to_ascii_uppercase()
            } else {
                b
            };
            n += 1;
        }
        if let Some(b) = pad() {
            variant[n] = b;
            n += 1;
        }
        assert!(same(normalized(&bytes[..len]), normalized(&variant[..n])));
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

    #[test]
    fn ascii_normalize_trims_every_whitespace_trim_does() {
        let input = "\t\n\x0b\x0c\r A@B.C \x0b";
        assert_eq!(normalize(input), input.trim().to_lowercase());
        assert_eq!(normalize(input), "a@b.c");
    }

    proptest! {
        // The ASCII fast path is exactly `trim().to_lowercase()`, which the
        // Kani proofs rely on (design D7). Any ASCII, all whitespace kinds.
        #[test]
        fn prop_ascii_normalize_matches_trim_to_lowercase(input in "[\\x00-\\x7f]{0,24}") {
            prop_assert_eq!(normalize(&input), input.trim().to_lowercase());
        }

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
