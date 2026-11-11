//! A requested, not yet confirmed, change of a user's email address.

use super::Email;
use chrono::{DateTime, Utc};

/// A requested, not yet confirmed, change of a user's email address.
///
/// The new address and the time of the request only exist together, so they
/// live in one value: a user either has a pending change or does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEmailChange {
    new_email: Email,
    requested_at: DateTime<Utc>,
}

impl PendingEmailChange {
    /// A change to `new_email`, requested at `requested_at`.
    #[must_use]
    pub const fn new(new_email: Email, requested_at: DateTime<Utc>) -> Self {
        Self {
            new_email,
            requested_at,
        }
    }

    /// Builds the change from its two persisted columns: present only when both
    /// are set.
    #[must_use]
    pub fn from_columns(
        new_email: Option<Email>,
        requested_at: Option<DateTime<Utc>>,
    ) -> Option<Self> {
        new_email
            .zip(requested_at)
            .map(|(new_email, requested_at)| Self::new(new_email, requested_at))
    }

    /// The address the user wants to change to.
    #[must_use]
    pub const fn new_email(&self) -> &Email {
        &self.new_email
    }

    /// When the change was requested.
    #[must_use]
    pub const fn requested_at(&self) -> DateTime<Utc> {
        self.requested_at
    }

    /// Consumes the change, returning the new address.
    #[must_use]
    pub fn into_new_email(self) -> Email {
        self.new_email
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_columns_needs_both() {
        let email = || Some(Email::new("new@example.com").unwrap());
        let now = Utc::now();
        assert!(PendingEmailChange::from_columns(email(), Some(now)).is_some());
        assert!(PendingEmailChange::from_columns(email(), None).is_none());
        assert!(PendingEmailChange::from_columns(None, Some(now)).is_none());
    }
}
