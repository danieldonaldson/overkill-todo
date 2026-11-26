//! Shared rules for the Member-visible single-line strings (list names and
//! todo titles).
//!
//! A value is trimmed, then counted in Unicode code points (never UTF-16 units
//! or bytes), and must not contain control characters or the two Unicode line
//! and paragraph separators. The two types below are the only way a name or
//! title reaches an entity, so every write path applies the same rule.

use crate::errors::DomainError;

/// What the single-line rule decides for some text, counted in code points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SingleLine {
    /// Nothing but whitespace.
    Empty,
    /// More than `max` code points once trimmed.
    TooLong,
    /// A control character, U+2028 or U+2029 inside the trimmed text.
    NotOneLine,
    /// Accepted: the trimmed text is `chars[start..end]`.
    Valid { start: usize, end: usize },
}

/// The rule itself, over code points and without allocating, so Kani can
/// prove it (design D13). Trims as [`str::trim`] does, with
/// [`char::is_whitespace`], then counts what is left.
pub(crate) fn check_single_line(chars: &[char], max: usize) -> SingleLine {
    let mut start = 0;
    while start < chars.len() && chars[start].is_whitespace() {
        start += 1;
    }
    let mut end = chars.len();
    while end > start && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if start == end {
        SingleLine::Empty
    } else if end - start > max {
        SingleLine::TooLong
    } else if chars[start..end]
        .iter()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        SingleLine::NotOneLine
    } else {
        SingleLine::Valid { start, end }
    }
}

/// Trims `text` and validates it as one line of `1..=max` code points.
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
    let chars: Vec<char> = text.chars().collect();
    match check_single_line(&chars, max) {
        SingleLine::Empty => Err(DomainError::Validation(format!("{field} can't be empty"))),
        SingleLine::TooLong => Err(DomainError::Validation(format!(
            "{field} must be at most {max} characters"
        ))),
        SingleLine::NotOneLine => Err(DomainError::Validation(format!(
            "{field} must be a single line without control characters"
        ))),
        SingleLine::Valid { start, end } => Ok(chars[start..end].iter().collect()),
    }
}

/// Kani proofs (`just verify`). Bounds and assumptions are on each harness.
#[cfg(kani)]
mod verification {
    use super::*;

    /// Spec: text is trimmed of whitespace at both ends, what is left is counted
    /// in code points, and it is refused when empty, longer than `max`, or
    /// holding a control character (U+0000 to U+001F, U+007F to U+009F),
    /// U+2028 or U+2029. Stated with the first and last non-whitespace code
    /// points and the ranges written out, not with the kernel's loops.
    /// Bound: up to 6 symbolic `char`s, any valid code points, and `max <= 4`,
    /// so every outcome is reached; the proptest below ties the kernel to
    /// `str::trim` at 100 and 500. Assumes `char::is_whitespace` is the
    /// definition of whitespace, as `str::trim` uses it.
    #[kani::proof]
    #[kani::unwind(8)]
    fn the_single_line_rule_trims_counts_code_points_and_refuses_line_breaks() {
        let chars: [char; 6] = kani::any();
        let len: usize = kani::any();
        let max: usize = kani::any();
        kani::assume(len <= 6 && max <= 4);
        let text = &chars[..len];

        let refused = |c: char| {
            let u = u32::from(c);
            u <= 0x1f || (0x7f..=0x9f).contains(&u) || u == 0x2028 || u == 0x2029
        };
        let mut first = None;
        let mut last = 0;
        for (i, c) in text.iter().enumerate() {
            if !c.is_whitespace() {
                first = first.or(Some(i));
                last = i;
            }
        }
        let expected = match first {
            None => SingleLine::Empty,
            Some(first) if last + 1 - first > max => SingleLine::TooLong,
            Some(first) if text[first..=last].iter().any(|c| refused(*c)) => SingleLine::NotOneLine,
            Some(first) => SingleLine::Valid {
                start: first,
                end: last + 1,
            },
        };
        assert_eq!(check_single_line(text, max), expected);
    }
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

    /// The rule as it was written before the kernel, with `str::trim`: the
    /// proptest's oracle.
    fn oracle(text: &str, max: usize) -> Result<String, &'static str> {
        let trimmed = text.trim();
        let count = trimmed.chars().count();
        if count == 0 {
            Err("empty")
        } else if count > max {
            Err("too long")
        } else if trimmed
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        {
            Err("not one line")
        } else {
            Ok(trimmed.to_string())
        }
    }

    fn verdict(result: Result<String, DomainError>) -> Result<String, &'static str> {
        result.map_err(|err| {
            let message = err.to_string();
            if message.contains("can't be empty") {
                "empty"
            } else if message.contains("at most") {
                "too long"
            } else {
                "not one line"
            }
        })
    }

    proptest::proptest! {
        /// Ties the kernel Kani proves (6 code points) to `str::trim` and
        /// `chars().count()` at the real limits, around both of them.
        #[test]
        fn the_kernel_agrees_with_str_trim_at_100_and_500(
            text in proptest::string::string_regex(
                "[ \t\n\r\u{a0}\u{2000}\u{3000}a-z😀é\u{301}\u{0}\u{7f}\u{85}\u{9f}\u{2028}\u{2029}]{0,520}"
            ).unwrap(),
            pad in 0usize..4,
        ) {
            let text = format!("{}{text}{}", " ".repeat(pad), "\u{3000}".repeat(pad));
            for max in [100, 500] {
                proptest::prop_assert_eq!(
                    verdict(normalize_single_line(&text, max, "Field")),
                    oracle(&text, max)
                );
            }
        }

        /// Mostly letters, so lengths around both limits are common.
        #[test]
        fn the_kernel_agrees_with_str_trim_near_the_limits(
            body in "[a-z😀]{95,105}|[a-z😀]{495,505}",
            left in "[ \t\u{a0}]{0,3}",
            right in "[ \n\u{2003}]{0,3}",
        ) {
            let text = format!("{left}{body}{right}");
            for max in [100, 500] {
                proptest::prop_assert_eq!(
                    verdict(normalize_single_line(&text, max, "Field")),
                    oracle(&text, max)
                );
            }
        }
    }

    #[test]
    fn error_never_repeats_the_text() {
        let err = normalize_single_line("secret\nvalue", 100, "List name").unwrap_err();
        assert!(!err.to_string().contains("secret"));
    }
}
