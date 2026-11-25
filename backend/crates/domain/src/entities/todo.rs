//! A todo inside a list.

use jiff::Timestamp;

use crate::errors::DomainError;
use crate::value_objects::{ListId, TodoId, TodoTitle};

/// Most todos one list may hold, open and done together (design D5).
pub const MAX_TODOS_PER_LIST: usize = 1000;

/// Spacing between adjacent open-todo positions (design D6).
///
/// A move inserts the midpoint of its neighbours; renumbering only happens when
/// that gap runs out.
pub const POSITION_GAP: i64 = 1 << 20;

/// The position for a todo appended after `last_open`.
///
/// `None` means the list has no open todo. A new todo and a todo marked not
/// done both go to the bottom, so both take this value under the list lock.
#[must_use]
pub const fn append_position(last_open: Option<i64>) -> i64 {
    match last_open {
        Some(position) => position.saturating_add(POSITION_GAP),
        None => POSITION_GAP,
    }
}

/// A todo inside exactly one list.
///
/// An open todo has a `position` and no `completed_at`; a done todo has a
/// completion time and no position. [`Todo::from_persistence`] refuses any
/// other combination, mirroring the database check (design D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    id: TodoId,
    list_id: ListId,
    title: TodoTitle,
    version: i64,
    position: Option<i64>,
    completed_at: Option<Timestamp>,
    created_at: Timestamp,
}

/// The full set of a [`Todo`]'s fields in named form.
pub struct TodoParts {
    pub id: TodoId,
    pub list_id: ListId,
    pub title: TodoTitle,
    pub version: i64,
    pub position: Option<i64>,
    pub completed_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

impl Todo {
    /// Creates a new open todo at `position`, version 1.
    #[must_use]
    pub fn new_open(list_id: ListId, title: TodoTitle, position: i64, now: Timestamp) -> Self {
        Self {
            id: TodoId::new(),
            list_id,
            title,
            version: 1,
            position: Some(position),
            completed_at: None,
            created_at: now,
        }
    }

    /// Reconstructs a todo from persistence.
    ///
    /// # Errors
    /// [`DomainError::Persistence`] when the open/done columns disagree (a row
    /// the database check should have refused).
    pub fn from_persistence(parts: TodoParts) -> Result<Self, DomainError> {
        if parts.position.is_some() == parts.completed_at.is_some() {
            return Err(DomainError::Persistence(
                "todo row mixes open and done columns".into(),
            ));
        }
        Ok(Self {
            id: parts.id,
            list_id: parts.list_id,
            title: parts.title,
            version: parts.version,
            position: parts.position,
            completed_at: parts.completed_at,
            created_at: parts.created_at,
        })
    }

    #[must_use]
    pub const fn id(&self) -> &TodoId {
        &self.id
    }

    #[must_use]
    pub const fn list_id(&self) -> &ListId {
        &self.list_id
    }

    #[must_use]
    pub const fn title(&self) -> &TodoTitle {
        &self.title
    }

    #[must_use]
    pub const fn version(&self) -> i64 {
        self.version
    }

    #[must_use]
    pub const fn position(&self) -> Option<i64> {
        self.position
    }

    #[must_use]
    pub const fn completed_at(&self) -> Option<Timestamp> {
        self.completed_at
    }

    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }

    #[must_use]
    pub const fn is_done(&self) -> bool {
        self.completed_at.is_some()
    }

    /// Marks the todo done, bumping its version.
    pub const fn mark_done(&mut self, now: Timestamp) {
        self.position = None;
        self.completed_at = Some(now);
        self.version += 1;
    }

    /// Marks the todo not done at `position`, bumping its version.
    pub const fn mark_not_done(&mut self, position: i64, _now: Timestamp) {
        self.position = Some(position);
        self.completed_at = None;
        self.version += 1;
    }

    /// Renames the todo, bumping its version.
    ///
    /// # Errors
    /// Propagates the title validation from [`TodoTitle::new`].
    pub fn rename(&mut self, title: &str) -> Result<(), DomainError> {
        self.title = TodoTitle::new(title)?;
        self.version += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value_objects::TodoTitle;
    use proptest::prelude::*;

    fn todo() -> Todo {
        Todo::new_open(
            ListId::new(),
            TodoTitle::new("Buy milk").unwrap(),
            POSITION_GAP,
            Timestamp::now(),
        )
    }

    #[test]
    fn new_is_open_at_version_one() {
        let todo = todo();
        assert!(!todo.is_done());
        assert_eq!(todo.version(), 1);
        assert_eq!(todo.position(), Some(POSITION_GAP));
    }

    #[test]
    fn append_position_starts_at_the_gap_and_increases() {
        assert_eq!(append_position(None), POSITION_GAP);
        assert_eq!(append_position(Some(POSITION_GAP)), 2 * POSITION_GAP);
        assert_eq!(append_position(Some(POSITION_GAP * 10)), POSITION_GAP * 11);
    }

    #[test]
    fn marking_done_moves_it_and_bumps_the_version() {
        let mut todo = todo();
        todo.mark_done(Timestamp::now());
        assert!(todo.is_done());
        assert_eq!(todo.position(), None);
        assert_eq!(todo.version(), 2);
    }

    #[test]
    fn marking_not_done_returns_it_to_the_bottom() {
        let mut todo = todo();
        todo.mark_done(Timestamp::now());
        todo.mark_not_done(5 * POSITION_GAP, Timestamp::now());
        assert!(!todo.is_done());
        assert_eq!(todo.position(), Some(5 * POSITION_GAP));
        assert_eq!(todo.version(), 3);
    }

    #[test]
    fn from_persistence_refuses_a_mixed_row() {
        let base = TodoParts {
            id: TodoId::new(),
            list_id: ListId::new(),
            title: TodoTitle::new("x").unwrap(),
            version: 1,
            position: Some(POSITION_GAP),
            completed_at: Some(Timestamp::now()),
            created_at: Timestamp::now(),
        };
        assert!(Todo::from_persistence(base).is_err());
    }

    proptest! {
        #[test]
        fn positions_stay_strictly_increasing(last in 0i64..(i64::MAX / 4)) {
            prop_assert!(append_position(Some(last)) > last);
        }
    }
}
