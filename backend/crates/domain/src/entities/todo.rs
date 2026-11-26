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
///
/// Strictly increasing unless `last_open` is within one gap of `i64::MAX`,
/// where it saturates. That is out of reach: no write raises a list's highest
/// open position by more than one gap, and renumbering puts it at most 65,537
/// gaps up (both proved below), so saturating takes about 2^43 writes to one
/// list, 27 years at 10,000 a second. Were it reached, the saturated position
/// would equal the last one and the deferrable unique constraint on positions
/// would refuse the write (design D2), so the order can't be corrupted.
#[must_use]
pub const fn append_position(last_open: Option<i64>) -> i64 {
    match last_open {
        Some(position) => position.saturating_add(POSITION_GAP),
        None => POSITION_GAP,
    }
}

/// Whether a todo is open or done (design D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoState {
    Open,
    Done,
}

/// A todo's state from which of its two columns are set.
///
/// An open todo has a position, a done one a completion time. `None` for both
/// or neither, a row the database check refuses. Over the columns' presence
/// only, so Kani can prove it without `jiff` (design D13).
#[must_use]
pub const fn todo_state(has_position: bool, has_completed_at: bool) -> Option<TodoState> {
    match (has_position, has_completed_at) {
        (true, false) => Some(TodoState::Open),
        (false, true) => Some(TodoState::Done),
        _ => None,
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
        if todo_state(parts.position.is_some(), parts.completed_at.is_some()).is_none() {
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
}

/// Kani proofs (`just verify`). Bounds and assumptions are on each harness.
#[cfg(kani)]
mod verification {
    use super::super::todo_order::{position_between, renumbered_other, renumbered_position};
    use super::*;

    /// 2^20, the design's literal gap (D6), written out rather than taken
    /// from `POSITION_GAP`.
    const GAP: i128 = 1_048_576;

    /// Spec: an appended todo goes one gap below the last open one, so
    /// positions strictly increase, and the first goes one gap from zero.
    /// Within one gap of `i64::MAX` it saturates there instead. Every `i64`
    /// last position, or none; no assumptions. No overflow or panic.
    #[kani::proof]
    fn appending_goes_one_gap_below_the_last_or_saturates() {
        let last: Option<i64> = kani::any();
        let position = append_position(last);
        match last {
            None => assert_eq!(i128::from(position), GAP),
            Some(last) if i128::from(last) + GAP <= i128::from(i64::MAX) => {
                assert_eq!(i128::from(position), i128::from(last) + GAP);
                assert!(position > last, "not strictly increasing");
            }
            Some(_) => assert_eq!(position, i64::MAX),
        }
    }

    /// Spec (why saturation is out of reach): no write puts a todo more than
    /// one gap above the list's highest open position (or above zero, for an
    /// empty list), and renumbering puts every todo at most 65,537 gaps up.
    /// Every `i64` highest position and neighbours at or below it, either
    /// absent; every `u16` rank and slot (a list holds at most 1,000 todos).
    /// Assumes these are the only writes of a position, as in the repository:
    /// an add or mark not done appends, a move uses `position_between` or
    /// renumbers.
    #[kani::proof]
    fn no_write_raises_the_highest_position_by_more_than_one_gap() {
        let highest: i64 = kani::any();
        let ceiling = i128::from(highest.max(0)) + GAP;

        let last: Option<i64> = kani::any();
        kani::assume(last.is_none_or(|last| last <= highest));
        assert!(i128::from(append_position(last)) <= ceiling);

        let (above, below): (Option<i64>, Option<i64>) = (kani::any(), kani::any());
        kani::assume(above.is_none_or(|above| above <= highest));
        kani::assume(below.is_none_or(|below| below <= highest));
        if let Some(position) = position_between(above, below) {
            assert!(i128::from(position) <= ceiling);
        }

        let (rank, slot): (u16, u16) = (kani::any(), kani::any());
        assert!(i128::from(renumbered_other(rank, slot)) <= 65_537 * GAP);
        assert!(i128::from(renumbered_position(u32::from(slot))) <= 65_537 * GAP);
    }

    /// Spec: a todo is open or done, never both or neither; open exactly when
    /// it has a position and no completion time, done exactly when it has a
    /// completion time and no position. Every combination of the two columns'
    /// presence; no assumptions. `from_persistence` refuses exactly the rows
    /// this returns `None` for (by reading: Kani can't build its `jiff` and
    /// `Uuid` fields).
    #[kani::proof]
    fn a_todo_is_open_or_done() {
        let (has_position, has_completed_at): (bool, bool) = (kani::any(), kani::any());
        let state = todo_state(has_position, has_completed_at);
        assert_eq!(state.is_some(), has_position ^ has_completed_at);
        assert_eq!(
            state == Some(TodoState::Open),
            has_position && !has_completed_at
        );
        assert_eq!(
            state == Some(TodoState::Done),
            has_completed_at && !has_position
        );
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

    #[test]
    fn from_persistence_refuses_a_row_neither_open_nor_done() {
        let parts = TodoParts {
            id: TodoId::new(),
            list_id: ListId::new(),
            title: TodoTitle::new("x").unwrap(),
            version: 1,
            position: None,
            completed_at: None,
            created_at: Timestamp::now(),
        };
        assert!(Todo::from_persistence(parts).is_err());
    }

    proptest! {
        #[test]
        fn positions_stay_strictly_increasing(last in 0i64..(i64::MAX / 4)) {
            prop_assert!(append_position(Some(last)) > last);
        }
    }
}
