//! Where a user account is in its deletion lifecycle.

use chrono::{DateTime, Utc};

/// Where a user account is in its deletion lifecycle.
///
/// ```text
///            schedule                 grace period elapses
///   Active ───────────▶ PendingDeletion ─────────────────▶ Deleted
///      ▲                     │                               (terminal)
///      └──────── cancel ─────┘
/// ```
///
/// Persisted as two nullable columns (`deleted_at`, `deletion_scheduled_at`);
/// this type is the only place that decides what a pair of them means, so a
/// deleted account can never also be pending deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountStatus {
    /// A normal, live account.
    #[default]
    Active,
    /// Deletion requested; the account is purged once `scheduled_for` passes
    /// unless the user cancels first.
    PendingDeletion {
        /// When the grace period ends.
        scheduled_for: DateTime<Utc>,
    },
    /// Anonymized. Terminal: no transition leaves this state.
    Deleted {
        /// When the account was anonymized.
        deleted_at: DateTime<Utc>,
    },
}

impl AccountStatus {
    /// Builds the status from its two persisted columns.
    ///
    /// `deleted_at` wins when both are set: deletion is terminal, so a schedule
    /// left beside it is stale and is dropped (the next save writes it back as
    /// `NULL`).
    #[must_use]
    pub const fn from_columns(
        deleted_at: Option<DateTime<Utc>>,
        deletion_scheduled_at: Option<DateTime<Utc>>,
    ) -> Self {
        match (deleted_at, deletion_scheduled_at) {
            (Some(deleted_at), _) => Self::Deleted { deleted_at },
            (None, Some(scheduled_for)) => Self::PendingDeletion { scheduled_for },
            (None, None) => Self::Active,
        }
    }

    /// The `deleted_at` column value.
    #[must_use]
    pub const fn deleted_at(self) -> Option<DateTime<Utc>> {
        match self {
            Self::Deleted { deleted_at } => Some(deleted_at),
            Self::Active | Self::PendingDeletion { .. } => None,
        }
    }

    /// The `deletion_scheduled_at` column value.
    #[must_use]
    pub const fn deletion_scheduled_at(self) -> Option<DateTime<Utc>> {
        match self {
            Self::PendingDeletion { scheduled_for } => Some(scheduled_for),
            Self::Active | Self::Deleted { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_columns_maps_each_combination() {
        let t = Utc::now();
        assert_eq!(
            AccountStatus::from_columns(None, None),
            AccountStatus::Active
        );
        assert_eq!(
            AccountStatus::from_columns(None, Some(t)),
            AccountStatus::PendingDeletion { scheduled_for: t }
        );
        assert_eq!(
            AccountStatus::from_columns(Some(t), None),
            AccountStatus::Deleted { deleted_at: t }
        );
    }

    #[test]
    fn deleted_wins_over_a_stale_schedule() {
        let deleted = Utc::now();
        let scheduled = deleted + chrono::Duration::days(1);
        let status = AccountStatus::from_columns(Some(deleted), Some(scheduled));
        assert_eq!(
            status,
            AccountStatus::Deleted {
                deleted_at: deleted
            }
        );
        assert_eq!(status.deletion_scheduled_at(), None);
    }

    #[test]
    fn columns_round_trip() {
        let t = Utc::now();
        for status in [
            AccountStatus::Active,
            AccountStatus::PendingDeletion { scheduled_for: t },
            AccountStatus::Deleted { deleted_at: t },
        ] {
            assert_eq!(
                AccountStatus::from_columns(status.deleted_at(), status.deletion_scheduled_at()),
                status
            );
        }
    }
}
