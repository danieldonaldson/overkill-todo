//! Shared rules for the Member-visible single-line strings (list names and
//! todo titles).
//!
//! A value is trimmed, then counted in Unicode code points (never UTF-16 units
//! or bytes), and must not contain control characters or the two Unicode line
//! and paragraph separators. The two types below are the only way a name or
//! title reaches an entity, so every write path applies the same rule.

use crate::errors::DomainError;

/// Trims `text` and validates it as one line of `min..=max` code points.
///
/// # Errors
/// [`DomainError::Validation`] when the trimmed text is empty, too long, or
/// holds a control character, U+2028 or U+2029. The message names the field
/// and its limit; it never repeats the text.
pub(crate) fn normalize_single_line(
    text: &str,
    max: usize,
    field: &str,
) -> Result<String, DomainError> {
    let trimmed = text.trim();
    let chars = trimmed.chars().count();
    if chars == 0 {
        return Err(DomainError::Validation(format!("{field} can't be empty")));
    }
    if chars > max {
        return Err(DomainError::Validation(format!(
            "{field} must be at most {max} characters"
        )));
    }
    if trimmed
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err(DomainError::Validation(format!(
            "{field} must be a single line without control characters"
        )));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            normalize_single_line("  Work \t\n", 100, "List name").unwrap(),
            "Work"
        );
    }

    #[test]
    fn counts_code_points_not_utf16_units() {
        // 100 emoji: 400 UTF-16 units, 100 code points.
        let emoji = "😀".repeat(100);
        assert!(normalize_single_line(&emoji, 100, "List name").is_ok());
    }

    #[test]
    fn combining_marks_count_as_characters() {
        let combining = "e\u{301}".repeat(50);
        assert!(normalize_single_line(&combining, 100, "List name").is_ok());
        assert!(normalize_single_line(&format!("{combining}e\u{301}"), 100, "List name").is_err());
    }

    #[test]
    fn refuses_control_characters_and_separators() {
        for bad in ["a\nb", "a\0b", "a\u{2028}b", "a\u{2029}b", "a\u{7f}b"] {
            assert!(
                normalize_single_line(bad, 100, "List name").is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn whitespace_only_is_empty() {
        let err = normalize_single_line(" \t\n", 100, "List name").unwrap_err();
        assert!(err.to_string().contains("can't be empty"));
    }

    #[test]
    fn error_never_repeats_the_text() {
        let err = normalize_single_line("secret\nvalue", 100, "List name").unwrap_err();
        assert!(!err.to_string().contains("secret"));
    }
}
