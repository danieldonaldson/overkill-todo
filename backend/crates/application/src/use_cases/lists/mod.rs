//! Use cases for a Member's lists and todos.

use domain::errors::DomainError;

pub mod add_todo;
pub mod create_list;
pub mod get_list;
pub mod list_lists;
pub mod mark_todo;

pub use add_todo::AddTodoUseCase;
pub use create_list::CreateListUseCase;
pub use get_list::GetListUseCase;
pub use list_lists::ListListsUseCase;
pub use mark_todo::MarkTodoUseCase;

/// A write result the handler can turn into a status: applied, refused as
/// stale (with the current row), or refused at a limit.
#[derive(Debug)]
pub enum MutationOutcome<T> {
    Applied(T),
    Stale(T),
    LimitReached(crate::ports::outgoing::LimitKind),
}

/// Why a list or todo operation failed with no outcome.
///
/// `ListNotFound` and `TodoNotFound` are distinct HTTP codes, and both are used
/// for another Member's IDs exactly as for unknown ones (security P-35).
#[derive(Debug)]
pub enum ListError {
    ListNotFound,
    TodoNotFound,
    Domain(DomainError),
}

impl std::fmt::Display for ListError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ListNotFound => f.write_str("List not found"),
            Self::TodoNotFound => f.write_str("Todo not found"),
            Self::Domain(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ListError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Domain(error) => Some(error),
            Self::ListNotFound | Self::TodoNotFound => None,
        }
    }
}

impl From<DomainError> for ListError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}
