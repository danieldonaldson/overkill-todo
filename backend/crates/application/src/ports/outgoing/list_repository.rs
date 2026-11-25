//! List repository port (driven port).
//!
//! One port for a Member's lists and their todos. Every write takes the owning
//! `user_id`: a list that isn't the caller's is indistinguishable from a missing
//! one, and the adapter locks it before touching its todos (design D4).

use async_trait::async_trait;
use domain::entities::{List, Todo};
use domain::errors::DomainError;
use domain::value_objects::{ListId, ListName, TodoId, TodoTitle, UserId};
use uuid::Uuid;

/// A list with the counts every list answer carries.
#[derive(Debug, Clone)]
pub struct ListSummary {
    pub list: List,
    pub open_count: i64,
    pub todo_count: i64,
}

/// A list and its todos: open ones by position, then done ones most recently
/// completed first (design D5).
#[derive(Debug, Clone)]
pub struct ListWithTodos {
    pub list: List,
    pub open_count: i64,
    pub todo_count: i64,
    pub todos: Vec<Todo>,
}

/// The limit an addition would cross.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    Lists,
    Todos,
}

impl LimitKind {
    /// The Member-facing message naming the limit.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Lists => "You can have at most 100 lists",
            Self::Todos => "A list can hold at most 1,000 todos",
        }
    }
}

/// The answer to a write: applied, already applied by an earlier attempt (the
/// same client key), refused as stale, refused at a limit, or missing.
///
/// `Repeated` is success: a lost answer retried with the same key must not
/// create a second row (design D3).
#[derive(Debug, Clone)]
pub enum ListWrite<T> {
    Applied(T),
    Repeated(T),
    Stale(T),
    LimitReached(LimitKind),
    ListNotFound,
    TodoNotFound,
}

impl<T> ListWrite<T> {
    /// The row in a successful answer.
    #[must_use]
    pub fn into_applied(self) -> Option<T> {
        match self {
            Self::Applied(value) | Self::Repeated(value) => Some(value),
            Self::Stale(_) | Self::LimitReached(_) | Self::ListNotFound | Self::TodoNotFound => {
                None
            }
        }
    }
}

/// Storage for a Member's lists and todos.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ListRepository: Send + Sync {
    /// Every list of `user_id`, oldest first, with its open and total counts.
    async fn list_summaries(&self, user_id: &UserId) -> Result<Vec<ListSummary>, DomainError>;

    /// One list of `user_id` with all its todos in contract order.
    async fn find_list(
        &self,
        user_id: &UserId,
        list_id: &ListId,
    ) -> Result<Option<ListWithTodos>, DomainError>;

    /// Creates a list, or returns the existing one for a repeated `key`.
    ///
    /// Locks the account row so the list limit holds under concurrent requests
    /// and a purge can't be followed by a new list.
    async fn create_list(
        &self,
        user_id: &UserId,
        key: Uuid,
        name: &ListName,
    ) -> Result<ListWrite<ListSummary>, DomainError>;

    /// Adds a todo, or returns the existing one for a repeated `key`.
    async fn add_todo(
        &self,
        user_id: &UserId,
        list_id: &ListId,
        key: Uuid,
        title: &TodoTitle,
    ) -> Result<ListWrite<Todo>, DomainError>;

    /// Applies a done-state change at `version`.
    ///
    /// The same state already stored succeeds without changing the version;
    /// any other state at a different version is `Stale(current)`.
    async fn mark_todo(
        &self,
        user_id: &UserId,
        list_id: &ListId,
        todo_id: &TodoId,
        version: i64,
        done: bool,
    ) -> Result<ListWrite<Todo>, DomainError>;

    /// The todos of one of `user_id`'s lists, for the streamed data export.
    async fn list_todos(
        &self,
        user_id: &UserId,
        list_id: &ListId,
    ) -> Result<Vec<Todo>, DomainError>;
}
