//! Profile management handlers — auth-relevant endpoints only (v1).

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header::HeaderMap},
};
use chrono::Utc;
use std::sync::Arc;

use crate::{
    application::{
        dto::{
            AccountDeletionResponse, AccountExportResponse, ChangePasswordRequest,
            LinkedOAuthAccountExport, RequestDeletionRequest, SetPasswordRequest,
        },
        use_cases::{
            CancelAccountDeletionUseCase, ChangePasswordUseCase, ExportUserDataUseCase,
            RequestAccountDeletionUseCase, SetPasswordUseCase,
        },
    },
    domain::{entities::User, errors::DomainError},
    infrastructure::api::error::AppError,
    infrastructure::api::handlers::session::extract_refresh_token_cookie,
    infrastructure::api::{extractors::ValidatedJson, middleware::AuthUser},
};

/// Dependencies for profile handlers.
pub struct ProfileHandlerDeps {
    pub change_password_use_case: Arc<ChangePasswordUseCase>,
    pub set_password_use_case: Arc<SetPasswordUseCase>,
    pub export_user_data_use_case: Arc<ExportUserDataUseCase>,
    pub request_account_deletion_use_case: Arc<RequestAccountDeletionUseCase>,
    pub cancel_account_deletion_use_case: Arc<CancelAccountDeletionUseCase>,
}

/// Change password (for users who already have a password)
#[utoipa::path(
    post,
    path = "/api/v1/users/password/change",
    tag = "Profile",
    request_body = ChangePasswordRequest,
    responses(
        (status = 204, description = "Password changed successfully"),
        (status = 401, description = "Not authenticated or wrong current password"),
        (status = 404, description = "User not found")
    ),
    security(("bearer_auth" = []))
)]
/// # Errors
///
/// Returns an error if the user is not found, current password is wrong, or validation fails.
#[tracing::instrument(name = "handler.change_password", skip(deps, request, headers))]
pub async fn change_password(
    State(deps): State<Arc<ProfileHandlerDeps>>,
    auth_user: AuthUser,
    headers: HeaderMap,
    ValidatedJson(request): ValidatedJson<ChangePasswordRequest>,
) -> Result<StatusCode, AppError> {
    // The current device's refresh token (from its httpOnly cookie) is preserved
    // so the user stays logged in here; every other session is revoked.
    let current_refresh_token = extract_refresh_token_cookie(&headers);

    deps.change_password_use_case
        .execute(
            &auth_user.user_id,
            &request.current_password,
            &request.new_password,
            current_refresh_token.as_deref(),
        )
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Set password (for OAuth-only users without a password)
#[utoipa::path(
    post,
    path = "/api/v1/users/password/set",
    tag = "Profile",
    request_body = SetPasswordRequest,
    responses(
        (status = 204, description = "Password set successfully"),
        (status = 400, description = "User already has password"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "User not found")
    ),
    security(("bearer_auth" = []))
)]
/// # Errors
///
/// Returns an error if the user is not found, already has a password, or validation fails.
#[tracing::instrument(name = "handler.set_password", skip(deps, request))]
pub async fn set_password(
    State(deps): State<Arc<ProfileHandlerDeps>>,
    auth_user: AuthUser,
    ValidatedJson(request): ValidatedJson<SetPasswordRequest>,
) -> Result<StatusCode, AppError> {
    deps.set_password_use_case
        .execute(&auth_user.user_id, &request.new_password)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Maps a user + their linked OAuth accounts to the export response DTO.
fn build_export(
    user: User,
    oauth: &[crate::domain::entities::OAuthAccount],
) -> AccountExportResponse {
    let linked_oauth_accounts = oauth
        .iter()
        .map(|a| LinkedOAuthAccountExport {
            provider: a.provider().as_str().to_string(),
            provider_email: a.provider_email().map(ToString::to_string),
            linked_at: a.linked_at(),
        })
        .collect();

    let has_password = user.has_password();
    let parts = user.into_parts();
    AccountExportResponse {
        id: *parts.id.as_uuid(),
        email: parts.email.into_string(),
        first_name: parts.first_name,
        last_name: parts.last_name,
        email_verified: parts.email_verified,
        has_password,
        pending_email: parts
            .pending_email
            .map(crate::domain::value_objects::Email::into_string),
        deletion_scheduled_at: parts.deletion_scheduled_at,
        role: parts.role.to_db_str().to_string(),
        created_at: parts.created_at,
        updated_at: parts.updated_at,
        linked_oauth_accounts,
        exported_at: Utc::now(),
    }
}

/// Export all personal data held about the authenticated user (GDPR).
#[utoipa::path(
    get,
    path = "/api/v1/users/me/export",
    tag = "Profile",
    responses(
        (status = 200, description = "Machine-readable export of the user's data", body = AccountExportResponse),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "User not found")
    ),
    security(("bearer_auth" = []))
)]
/// # Errors
///
/// Returns an error if the user is not found or a repository call fails.
#[tracing::instrument(name = "handler.export_user_data", skip(deps))]
pub async fn export_data(
    State(deps): State<Arc<ProfileHandlerDeps>>,
    auth_user: AuthUser,
) -> Result<Json<AccountExportResponse>, AppError> {
    let (user, oauth) = deps
        .export_user_data_use_case
        .execute(&auth_user.user_id)
        .await?;
    Ok(Json(build_export(user, &oauth)))
}

/// Request account deletion (soft, 14-day grace period).
#[utoipa::path(
    delete,
    path = "/api/v1/users/me",
    tag = "Profile",
    request_body = RequestDeletionRequest,
    responses(
        (status = 200, description = "Deletion scheduled", body = AccountDeletionResponse),
        (status = 400, description = "Missing confirmation or password"),
        (status = 401, description = "Not authenticated or wrong password"),
        (status = 404, description = "User not found")
    ),
    security(("bearer_auth" = []))
)]
/// # Errors
///
/// Returns an error if confirmation is missing, the password is wrong, or the
/// user is not found.
#[tracing::instrument(name = "handler.request_account_deletion", skip(deps, request))]
pub async fn request_deletion(
    State(deps): State<Arc<ProfileHandlerDeps>>,
    auth_user: AuthUser,
    ValidatedJson(request): ValidatedJson<RequestDeletionRequest>,
) -> Result<Json<AccountDeletionResponse>, AppError> {
    // Defence-in-depth: the client requires typing "DELETE"; enforce it server-side
    // too so a stray API call can't schedule deletion without explicit intent.
    if request.confirmation.as_deref() != Some("DELETE") {
        return Err(
            DomainError::Validation("Type DELETE to confirm account deletion".into()).into(),
        );
    }

    let scheduled_at = deps
        .request_account_deletion_use_case
        .execute(&auth_user.user_id, request.password.as_deref())
        .await?;

    Ok(Json(AccountDeletionResponse {
        deletion_scheduled_at: Some(scheduled_at),
    }))
}

/// Cancel a pending account deletion (during the grace period).
#[utoipa::path(
    post,
    path = "/api/v1/users/me/cancel-deletion",
    tag = "Profile",
    responses(
        (status = 200, description = "Deletion cancelled", body = AccountDeletionResponse),
        (status = 400, description = "Account already deleted"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "User not found")
    ),
    security(("bearer_auth" = []))
)]
/// # Errors
///
/// Returns an error if the account is already deleted or the user is not found.
#[tracing::instrument(name = "handler.cancel_account_deletion", skip(deps))]
pub async fn cancel_deletion(
    State(deps): State<Arc<ProfileHandlerDeps>>,
    auth_user: AuthUser,
) -> Result<Json<AccountDeletionResponse>, AppError> {
    deps.cancel_account_deletion_use_case
        .execute(&auth_user.user_id)
        .await?;

    Ok(Json(AccountDeletionResponse {
        deletion_scheduled_at: None,
    }))
}
