//! A Member's todo list.

use jiff::Timestamp;

use crate::errors::DomainError;
use crate::value_objects::{ListId, ListName, UserId};

/// Most lists one Member may have (design D5).
pub const MAX_LISTS_PER_USER: usize = 100;

/// A list belonging to exactly one Member.
///
/// The owning Member is part of every lookup; there is no list without an
/// owner (design D3/D4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    id: ListId,
    user_id: UserId,
    name: ListName,
    version: i64,
    created_at: Timestamp,
}

/// The full set of a [`List`]'s fields in named form.
pub struct ListParts {
    pub id: ListId,
    pub user_id: UserId,
    pub name: ListName,
    pub version: i64,
    pub created_at: Timestamp,
}

impl List {
    /// Creates a new list at version 1.
    #[must_use]
    pub fn new(user_id: UserId, name: ListName, now: Timestamp) -> Self {
        Self {
            id: ListId::new(),
            user_id,
            name,
            version: 1,
            created_at: now,
        }
    }

    /// Reconstructs a list from persistence.
    #[must_use]
    pub fn from_persistence(parts: ListParts) -> Self {
        let ListParts {
            id,
            user_id,
            name,
            version,
            created_at,
        } = parts;
        Self {
            id,
            user_id,
            name,
            version,
            created_at,
        }
    }

    #[must_use]
    pub const fn id(&self) -> &ListId {
        &self.id
    }

    #[must_use]
    pub const fn user_id(&self) -> &UserId {
        &self.user_id
    }

    #[must_use]
    pub const fn name(&self) -> &ListName {
        &self.name
    }

    /// The optimistic-concurrency version the list was loaded at.
    #[must_use]
    pub const fn version(&self) -> i64 {
        self.version
    }

    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }

    /// Renames the list, bumping its version.
    ///
    /// # Errors
    /// Propagates the name validation from [`ListName::new`].
    pub fn rename(&mut self, name: &str) -> Result<(), DomainError> {
        self.name = ListName::new(name)?;
        self.version += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value_objects::ListName;

    fn list() -> List {
        List::new(
            UserId::new(),
            ListName::new("Work").unwrap(),
            Timestamp::now(),
        )
    }

    #[test]
    fn new_starts_at_version_one() {
        let list = list();
        assert_eq!(list.version(), 1);
    }

    #[test]
    fn rename_bumps_the_version() {
        let mut list = list();
        list.rename("Job").unwrap();
        assert_eq!(list.name().as_str(), "Job");
        assert_eq!(list.version(), 2);
    }
}
