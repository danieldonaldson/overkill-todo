//! List names and todo titles: the two Member-visible single-line strings.

use crate::errors::DomainError;
use crate::value_objects::single_line::normalize_single_line;

/// Longest list name, in Unicode code points after trimming.
pub const MAX_LIST_NAME_CHARS: usize = 100;
/// Longest todo title, in Unicode code points after trimming.
pub const MAX_TODO_TITLE_CHARS: usize = 500;

/// A validated list name.
///
/// There is no `Display` and `Debug` hides the text, so a name can't leak into
/// a log line or a stack trace by accident (design D9).
#[derive(Clone, PartialEq, Eq)]
pub struct ListName(String);

impl ListName {
    /// Trims and validates a list name (1–100 code points, one line).
    ///
    /// # Errors
    /// [`DomainError::Validation`] when the name is blank, too long, or holds a
    /// control character or a line separator.
    pub fn new(text: &str) -> Result<Self, DomainError> {
        normalize_single_line(text, MAX_LIST_NAME_CHARS, "List name").map(Self)
    }

    /// The stored, trimmed name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the name, returning the stored text.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for ListName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ListName([redacted])")
    }
}

/// A validated todo title.
///
/// The same secrecy rules as [`ListName`] apply.
#[derive(Clone, PartialEq, Eq)]
pub struct TodoTitle(String);

impl TodoTitle {
    /// Trims and validates a todo title (1–500 code points, one line).
    ///
    /// # Errors
    /// [`DomainError::Validation`] when the title is blank, too long, or holds a
    /// control character or a line separator.
    pub fn new(text: &str) -> Result<Self, DomainError> {
        normalize_single_line(text, MAX_TODO_TITLE_CHARS, "Todo title").map(Self)
    }

    /// The stored, trimmed title.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the title, returning the stored text.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for TodoTitle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TodoTitle([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn list_name_limit_counts_code_points() {
        assert!(ListName::new(&"😀".repeat(MAX_LIST_NAME_CHARS)).is_ok());
        assert!(ListName::new(&"😀".repeat(MAX_LIST_NAME_CHARS + 1)).is_err());
    }

    #[test]
    fn todo_title_limit_counts_code_points() {
        assert!(TodoTitle::new(&"😀".repeat(MAX_TODO_TITLE_CHARS)).is_ok());
        assert!(TodoTitle::new(&"😀".repeat(MAX_TODO_TITLE_CHARS + 1)).is_err());
    }

    #[test]
    fn trimming_happens_before_counting() {
        let padded = format!("{}  ", "x".repeat(MAX_LIST_NAME_CHARS));
        assert!(ListName::new(&padded).is_ok());
    }

    #[test]
    fn debug_never_shows_the_text() {
        let name = ListName::new("Groceries").unwrap();
        let title = TodoTitle::new("Buy milk").unwrap();
        assert_eq!(format!("{name:?}"), "ListName([redacted])");
        assert_eq!(format!("{title:?}"), "TodoTitle([redacted])");
        assert!(!format!("{name:?}").contains("Groceries"));
        assert!(!format!("{title:?}").contains("Buy milk"));
    }

    fn single_line() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9 !@#$%^&*()_+=-]{1,100}"
            .prop_filter("a name must not be blank", |text| !text.trim().is_empty())
    }

    proptest! {
        #[test]
        fn normalization_is_idempotent(text in single_line()) {
            let name = ListName::new(&text).unwrap();
            let again = ListName::new(name.as_str()).unwrap();
            prop_assert_eq!(name, again);
        }

        #[test]
        fn surrounding_whitespace_does_not_affect_acceptance(text in single_line()) {
            let padded = format!("\n\t {text} \r\n");
            let trimmed = ListName::new(&text).unwrap();
            prop_assert_eq!(ListName::new(&padded).unwrap(), trimmed);
        }
    }
}
