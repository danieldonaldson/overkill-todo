//! Request and response shapes for lists and todos (design D5).
//!
//! Array order is the contract for todos: open ones by position, then done ones
//! most recently completed first. No answer carries a position.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// A list as every answer carries it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListResponse {
    pub id: Uuid,
    pub name: String,
    /// Optimistic-concurrency version the list was loaded at.
    pub version: i64,
    /// Todos that are not done.
    pub open_count: i64,
    /// Every todo, open and done.
    pub todo_count: i64,
}

/// A todo in contract order. `done` replaces the stored position.
#[derive(Debug, Serialize, ToSchema)]
pub struct TodoResponse {
    pub id: Uuid,
    pub title: String,
    /// Optimistic-concurrency version the todo was loaded at.
    pub version: i64,
    pub done: bool,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub completed_at: Option<Timestamp>,
}

/// One list and all its todos.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListDetailResponse {
    pub list: ListResponse,
    pub todos: Vec<TodoResponse>,
}

/// Create a list. `key` is the client's creation key: repeating the request
/// with the same key returns the first list instead of creating another.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateListRequest {
    /// Random per action; a retry reuses it.
    pub key: Uuid,
    /// 1–100 trimmed characters, one line.
    #[schema(
        min_length = 1,
        max_length = 100,
        pattern = "^\\S(?:[^\\u0000-\\u001F\\u007F\\u2028\\u2029]*\\S)?$",
        example = "Work"
    )]
    pub name: String,
}

/// Add a todo to a list. `key` is the client's creation key.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AddTodoRequest {
    /// Random per action; a retry reuses it.
    pub key: Uuid,
    /// 1–500 trimmed characters, one line.
    #[schema(
        min_length = 1,
        max_length = 500,
        pattern = "^\\S(?:[^\\u0000-\\u001F\\u007F\\u2028\\u2029]*\\S)?$",
        example = "Buy milk"
    )]
    pub title: String,
}

/// Change a todo. Exactly one of `title` and `done` will be accepted; slice 1
/// supports only `done`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum TodoPatchRequest {
    Done(DoneTodoPatch),
}

/// Mark a todo done or not done at the version the Member saw.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DoneTodoPatch {
    /// Version the client saw; a change at another version is refused.
    pub version: i64,
    pub done: bool,
}

/// A `409` for an edit based on a version that has since changed.
#[derive(Debug, Serialize, ToSchema)]
pub struct StaleTodoConflictResponse {
    /// Always `"stale"`.
    pub code: String,
    pub message: String,
    /// The todo as the server now holds it.
    pub current: TodoResponse,
}

/// A `409` for an addition beyond a limit.
#[derive(Debug, Serialize, ToSchema)]
pub struct LimitConflictResponse {
    /// Always `"limit_reached"`.
    pub code: String,
    /// Names the limit, e.g. "A list can hold at most 1,000 todos".
    pub message: String,
}

/// One list in the data export, with its todos in contract order.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListExport {
    pub id: Uuid,
    pub name: String,
    pub version: i64,
    pub created_at: Timestamp,
    pub todos: Vec<TodoExport>,
}

/// One todo in the data export.
#[derive(Debug, Serialize, ToSchema)]
pub struct TodoExport {
    pub id: Uuid,
    pub title: String,
    pub version: i64,
    pub done: bool,
    pub created_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub completed_at: Option<Timestamp>,
}
