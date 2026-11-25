//! List and todo handlers (design D5).
//!
//! Every route is authorised by the group's session and verified-email guards
//! and by passing the caller's ID into each use case. A foreign ID is answered
//! exactly like an unknown one (security P-35).

use std::sync::Arc;

use application::dto::{
    AddTodoRequest, CreateListRequest, LimitConflictResponse, ListDetailResponse, ListResponse,
    StaleTodoConflictResponse, TodoPatchRequest, TodoResponse,
};
use application::ports::outgoing::{LimitKind, ListSummary, ListWithTodos};
use application::use_cases::{
    AddTodoUseCase, CreateListUseCase, GetListUseCase, ListListsUseCase, MarkTodoUseCase,
    MutationOutcome,
};
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use domain::entities::{List, Todo};
use domain::value_objects::{ListId, TodoId};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    error::AppError,
    extractors::{ApiJson, ApiPath},
    middleware::AuthUser,
};

/// Dependencies for the list handlers.
pub struct ListHandlerDeps {
    pub list_lists: Arc<ListListsUseCase>,
    pub create_list: Arc<CreateListUseCase>,
    pub get_list: Arc<GetListUseCase>,
    pub add_todo: Arc<AddTodoUseCase>,
    pub mark_todo: Arc<MarkTodoUseCase>,
}

/// A list ID in the path.
#[derive(Debug, Deserialize)]
pub struct ListPath {
    pub id: Uuid,
}

/// A list and todo ID in the path.
#[derive(Debug, Deserialize)]
pub struct TodoPath {
    pub id: Uuid,
    pub todo_id: Uuid,
}

fn list_response(list: &List, open_count: i64, todo_count: i64) -> ListResponse {
    ListResponse {
        id: *list.id().as_uuid(),
        name: list.name().as_str().to_string(),
        version: list.version(),
        open_count,
        todo_count,
    }
}

fn summary_response(summary: &ListSummary) -> ListResponse {
    list_response(&summary.list, summary.open_count, summary.todo_count)
}

fn todo_response(todo: &Todo) -> TodoResponse {
    TodoResponse {
        id: *todo.id().as_uuid(),
        title: todo.title().as_str().to_string(),
        version: todo.version(),
        done: todo.is_done(),
        created_at: todo.created_at(),
        completed_at: todo.completed_at(),
    }
}

fn detail_response(detail: &ListWithTodos) -> ListDetailResponse {
    ListDetailResponse {
        list: list_response(&detail.list, detail.open_count, detail.todo_count),
        todos: detail.todos.iter().map(todo_response).collect(),
    }
}

fn limit_response(limit: LimitKind) -> Response {
    (
        StatusCode::CONFLICT,
        Json(LimitConflictResponse {
            code: "limit_reached".to_string(),
            message: limit.message().to_string(),
        }),
    )
        .into_response()
}

fn stale_todo_response(todo: &Todo) -> Response {
    (
        StatusCode::CONFLICT,
        Json(StaleTodoConflictResponse {
            code: "stale".to_string(),
            message: "This todo changed elsewhere".to_string(),
            current: todo_response(todo),
        }),
    )
        .into_response()
}

/// The caller's lists, oldest first.
#[utoipa::path(
    get,
    path = "/lists",
    tag = "Lists",
    responses(
        (status = 200, description = "The caller's lists, oldest first", body = Vec<ListResponse>),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Email address not verified")
    )
)]
/// # Errors
/// Returns an error if the repository fails.
#[tracing::instrument(name = "handler.list_lists", skip_all)]
pub async fn list_lists(
    State(deps): State<Arc<ListHandlerDeps>>,
    auth_user: AuthUser,
) -> Result<Json<Vec<ListResponse>>, AppError> {
    let lists = deps.list_lists.execute(&auth_user.user_id).await?;
    Ok(Json(lists.iter().map(summary_response).collect()))
}

/// Create a list.
#[utoipa::path(
    post,
    path = "/lists",
    tag = "Lists",
    request_body = CreateListRequest,
    responses(
        (status = 200, description = "The list, created or already present for this key", body = ListResponse),
        (status = 400, description = "Invalid name or missing field"),
        (status = 401, description = "Not authenticated, or the account was purged"),
        (status = 403, description = "Email address not verified"),
        (status = 409, description = "The list limit was reached", body = LimitConflictResponse)
    )
)]
/// # Errors
/// Returns an error for invalid input, a repository failure, or a purged account.
#[tracing::instrument(name = "handler.create_list", skip_all)]
pub async fn create_list(
    State(deps): State<Arc<ListHandlerDeps>>,
    auth_user: AuthUser,
    ApiJson(request): ApiJson<CreateListRequest>,
) -> Result<Response, AppError> {
    let outcome = deps
        .create_list
        .execute(&auth_user.user_id, request.key, &request.name)
        .await?;
    Ok(match outcome {
        MutationOutcome::Applied(summary) | MutationOutcome::Stale(summary) => {
            Json(summary_response(&summary)).into_response()
        }
        MutationOutcome::LimitReached(limit) => limit_response(limit),
    })
}

/// One list and all its todos.
#[utoipa::path(
    get,
    path = "/lists/{id}",
    tag = "Lists",
    params(("id" = Uuid, Path, description = "List ID")),
    responses(
        (status = 200, description = "The list and its todos in contract order", body = ListDetailResponse),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Email address not verified"),
        (status = 404, description = "List not found (`list_not_found`)")
    )
)]
/// # Errors
/// Returns `list_not_found` for a missing or foreign list.
#[tracing::instrument(name = "handler.get_list", skip_all)]
pub async fn get_list(
    State(deps): State<Arc<ListHandlerDeps>>,
    auth_user: AuthUser,
    ApiPath(path): ApiPath<ListPath>,
) -> Result<Json<ListDetailResponse>, AppError> {
    let detail = deps
        .get_list
        .execute(&auth_user.user_id, &ListId::from_uuid(path.id))
        .await?;
    Ok(Json(detail_response(&detail)))
}

/// Add a todo at the bottom of the list.
#[utoipa::path(
    post,
    path = "/lists/{id}/todos",
    tag = "Lists",
    params(("id" = Uuid, Path, description = "List ID")),
    request_body = AddTodoRequest,
    responses(
        (status = 200, description = "The todo, created or already present for this key", body = TodoResponse),
        (status = 400, description = "Invalid title or missing field"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Email address not verified"),
        (status = 404, description = "List not found (`list_not_found`)"),
        (status = 409, description = "The todo limit was reached", body = LimitConflictResponse)
    )
)]
/// # Errors
/// Returns `list_not_found` or invalid-input errors.
#[tracing::instrument(name = "handler.add_todo", skip_all)]
pub async fn add_todo(
    State(deps): State<Arc<ListHandlerDeps>>,
    auth_user: AuthUser,
    ApiPath(path): ApiPath<ListPath>,
    ApiJson(request): ApiJson<AddTodoRequest>,
) -> Result<Response, AppError> {
    let outcome = deps
        .add_todo
        .execute(
            &auth_user.user_id,
            &ListId::from_uuid(path.id),
            request.key,
            &request.title,
        )
        .await?;
    Ok(match outcome {
        MutationOutcome::Applied(todo) | MutationOutcome::Stale(todo) => {
            Json(todo_response(&todo)).into_response()
        }
        MutationOutcome::LimitReached(limit) => limit_response(limit),
    })
}

/// Mark a todo done or not done.
#[utoipa::path(
    patch,
    path = "/lists/{id}/todos/{todo_id}",
    tag = "Lists",
    params(
        ("id" = Uuid, Path, description = "List ID"),
        ("todo_id" = Uuid, Path, description = "Todo ID")
    ),
    request_body = TodoPatchRequest,
    responses(
        (status = 200, description = "The todo after the change", body = TodoResponse),
        (status = 400, description = "Missing version or malformed body"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Email address not verified"),
        (status = 404, description = "List or todo not found (`list_not_found` / `todo_not_found`)"),
        (status = 409, description = "The todo changed elsewhere", body = StaleTodoConflictResponse)
    )
)]
/// # Errors
/// Returns the 404 codes for a missing or foreign ID.
#[tracing::instrument(name = "handler.mark_todo", skip_all)]
pub async fn mark_todo(
    State(deps): State<Arc<ListHandlerDeps>>,
    auth_user: AuthUser,
    ApiPath(path): ApiPath<TodoPath>,
    ApiJson(request): ApiJson<TodoPatchRequest>,
) -> Result<Response, AppError> {
    let TodoPatchRequest::Done(done_patch) = request;
    let outcome = deps
        .mark_todo
        .execute(
            &auth_user.user_id,
            &ListId::from_uuid(path.id),
            &TodoId::from_uuid(path.todo_id),
            done_patch.version,
            done_patch.done,
        )
        .await?;
    Ok(match outcome {
        MutationOutcome::Applied(todo) => Json(todo_response(&todo)).into_response(),
        MutationOutcome::Stale(todo) => stale_todo_response(&todo),
        MutationOutcome::LimitReached(limit) => limit_response(limit),
    })
}
