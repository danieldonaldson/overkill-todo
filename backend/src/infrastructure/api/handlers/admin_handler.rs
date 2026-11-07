//! Admin panel handlers (`/api/v1/admin`).
//!
//! Every route is guarded by the [`AdminUser`] extractor (admin or super-admin),
//! except role management, which requires [`SuperAdminUser`].

use axum::{
    Json,
    extract::{Path, Query, State},
};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    application::{
        dto::{AdminUserListResponse, AdminUserResponse, ListUsersQuery, SetUserRoleRequest},
        use_cases::{ListUsersUseCase, SetUserRoleUseCase},
    },
    domain::value_objects::UserId,
    infrastructure::api::error::AppError,
    infrastructure::api::extractors::{AdminUser, SuperAdminUser, ValidatedJson},
};

/// Dependencies for admin handlers.
pub struct AdminHandlerDeps {
    pub list_users_use_case: Arc<ListUsersUseCase>,
    pub set_user_role_use_case: Arc<SetUserRoleUseCase>,
}

/// List users (paginated).
#[utoipa::path(
    get,
    path = "/api/v1/admin/users",
    tag = "Admin",
    params(
        ("limit" = Option<i64>, Query, description = "Max users (default 50, max 200)"),
        ("offset" = Option<i64>, Query, description = "Pagination offset")
    ),
    responses(
        (status = 200, description = "A page of users", body = AdminUserListResponse),
        (status = 403, description = "Not an admin")
    ),
    security(("bearer_auth" = []))
)]
/// # Errors
/// Returns an error if the user store is unreachable.
#[tracing::instrument(name = "handler.admin.list_users", skip(deps, _admin))]
pub async fn list_users(
    State(deps): State<Arc<AdminHandlerDeps>>,
    _admin: AdminUser,
    Query(query): Query<ListUsersQuery>,
) -> Result<Json<AdminUserListResponse>, AppError> {
    let (limit, offset) = query.paginated();

    let users = deps.list_users_use_case.execute(limit, offset).await?;
    Ok(Json(AdminUserListResponse {
        users: users.iter().map(AdminUserResponse::from).collect(),
        limit,
        offset,
    }))
}

/// Change a user's role (super-admin only).
#[utoipa::path(
    put,
    path = "/api/v1/admin/users/{id}/role",
    tag = "Admin",
    request_body = SetUserRoleRequest,
    params(("id" = Uuid, Path, description = "User id")),
    responses(
        (status = 200, description = "Updated user", body = AdminUserResponse),
        (status = 400, description = "Invalid role"),
        (status = 403, description = "Not a super-admin / insufficient authority"),
        (status = 404, description = "User not found")
    ),
    security(("bearer_auth" = []))
)]
/// # Errors
/// Returns an error on an invalid role, insufficient authority, or a missing user.
#[tracing::instrument(name = "handler.admin.set_user_role", skip(deps, super_admin, request))]
pub async fn set_user_role(
    State(deps): State<Arc<AdminHandlerDeps>>,
    super_admin: SuperAdminUser,
    Path(id): Path<Uuid>,
    ValidatedJson(request): ValidatedJson<SetUserRoleRequest>,
) -> Result<Json<AdminUserResponse>, AppError> {
    // The SuperAdminUser extractor guarantees the actor is a super-admin; pass
    // their id so the use case can block self-demotion.
    let user = deps
        .set_user_role_use_case
        .execute(
            crate::domain::value_objects::UserRole::SuperAdmin,
            &super_admin.user_id,
            &UserId::from_uuid(id),
            &request.role,
        )
        .await?;
    Ok(Json(AdminUserResponse::from(&user)))
}
