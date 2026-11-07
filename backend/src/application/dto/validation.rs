//! Shared validation helpers for DTOs.

use validator::ValidationError;

/// Maximum raw length accepted for a formatted verification code.
const MAX_FORMATTED_CODE_LEN: usize = 32;

/// Extracts only ASCII digits from a user-entered verification code.
///
/// This accepts common copy/paste formatting like spaces and dashes while keeping
/// the downstream use cases on the canonical six-digit code form.
#[must_use]
pub fn sanitize_verification_code(input: &str) -> String {
    input.chars().filter(char::is_ascii_digit).collect()
}

/// Validates that a string is exactly 6 ASCII digits (verification code format).
#[must_use]
pub fn is_valid_verification_code(code: &str) -> bool {
    code.len() == 6 && code.chars().all(|c| c.is_ascii_digit())
}

/// Validator function for use with `#[validate(custom)]` attribute.
///
/// # Errors
///
/// Returns a `ValidationError` if the code is not exactly 6 ASCII digits.
pub fn validate_verification_code(code: &str) -> Result<(), ValidationError> {
    if code.len() <= MAX_FORMATTED_CODE_LEN
        && is_valid_verification_code(&sanitize_verification_code(code))
    {
        Ok(())
    } else {
        Err(ValidationError::new("code").with_message("Code must be 6 digits".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_six_ascii_digits_are_valid(code in "[0-9]{6}") {
            prop_assert!(is_valid_verification_code(&code));
            prop_assert!(validate_verification_code(&code).is_ok());
        }

        #[test]
        fn prop_formatted_six_digit_codes_are_valid(
            d1 in "[0-9]",
            d2 in "[0-9]",
            d3 in "[0-9]",
            d4 in "[0-9]",
            d5 in "[0-9]",
            d6 in "[0-9]"
        ) {
            let formatted = format!("{d1}{d2}{d3}-{d4} {d5}{d6}");
            let expected = format!("{d1}{d2}{d3}{d4}{d5}{d6}");
            prop_assert_eq!(sanitize_verification_code(&formatted), expected);
            prop_assert!(validate_verification_code(&formatted).is_ok());
        }

        #[test]
        fn prop_wrong_length_is_invalid(code in "[0-9]{0,5}|[0-9]{7,20}") {
            prop_assert!(!is_valid_verification_code(&code));
            prop_assert!(validate_verification_code(&code).is_err());
        }

        #[test]
        fn prop_six_chars_with_non_digits_are_invalid(code in ".{6}") {
            if sanitize_verification_code(&code).len() != 6 {
                prop_assert!(!is_valid_verification_code(&code));
                prop_assert!(validate_verification_code(&code).is_err());
            }
        }

        #[test]
        fn prop_arbitrary_strings_only_valid_if_six_ascii_digits(s in "\\PC{0,20}") {
            let expected = sanitize_verification_code(&s).len() == 6;
            prop_assert_eq!(validate_verification_code(&s).is_ok(), expected);
        }

        #[test]
        fn prop_error_has_correct_code(s in "[^0-9]{6}") {
            let err = validate_verification_code(&s).unwrap_err();
            prop_assert_eq!(err.code.as_ref(), "code");
        }
    }
}
