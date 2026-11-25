//! Profile management handlers — auth-relevant endpoints only (v1).

use axum::{
    Json,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderValue, StatusCode, header::CONTENT_TYPE, header::HeaderMap},
    response::Response,
};
use jiff::Timestamp;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::Instrument;

use application::{
    dto::{
        AccountDeletionResponse, AccountExportResponse, ChangePasswordRequest,
        LinkedOAuthAccountExport, ListExport, RequestDeletionRequest, SecurityActivityQuery,
        SecurityActivityResponse, SetPasswordRequest, TodoExport,
    },
    use_cases::{
        CancelAccountDeletionUseCase, ChangePasswordUseCase, ExportUserDataUseCase,
        ListSecurityActivityUseCase, RequestAccountDeletionUseCase, SetPasswordUseCase,
    },
};
use domain::{entities::User, errors::DomainError};

use crate::{
    error::AppError,
    extractors::{ApiQuery, ValidatedJson},
    handlers::session::extract_refresh_token_cookie,
    middleware::AuthUser,
};

/// Dependencies for profile handlers.
pub struct ProfileHandlerDeps {
    pub change_password_use_case: Arc<ChangePasswordUseCase>,
    pub set_password_use_case: Arc<SetPasswordUseCase>,
    pub export_user_data_use_case: Arc<ExportUserDataUseCase>,
    pub request_account_deletion_use_case: Arc<RequestAccountDeletionUseCase>,
    pub cancel_account_deletion_use_case: Arc<CancelAccountDeletionUseCase>,
    pub list_security_activity_use_case: Arc<ListSecurityActivityUseCase>,
}

/// Change password (for users who already have a password)
#[utoipa::path(
    post,
    path = "/password/change",
    tag = "Profile",
    request_body = ChangePasswordRequest,
    responses(
        (status = 204, description = "Password changed successfully"),
        (status = 401, description = "Not authenticated or wrong current password"),
        (status = 404, description = "User not found")
    )
)]
/// # Errors
///
/// Returns an error if the user is not found, current password is wrong, or validation fails.
#[tracing::instrument(name = "handler.change_password", skip_all)]
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
    path = "/password/set",
    tag = "Profile",
    request_body = SetPasswordRequest,
    responses(
        (status = 204, description = "Password set successfully"),
        (status = 400, description = "User already has password"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "User not found")
    )
)]
/// # Errors
///
/// Returns an error if the user is not found, already has a password, or validation fails.
#[tracing::instrument(name = "handler.set_password", skip_all)]
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

fn build_export_header(
    user: User,
    oauth: &[domain::entities::OAuthAccount],
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
        deletion_scheduled_at: parts.status.deletion_scheduled_at(),
        role: parts.role.to_db_str().to_string(),
        created_at: parts.created_at,
        updated_at: parts.updated_at,
        linked_oauth_accounts,
        exported_at: Timestamp::now(),
        lists: Vec::new(),
    }
}

/// Export all personal data held about the authenticated user (GDPR).
#[utoipa::path(
    get,
    path = "/me/export",
    tag = "Profile",
    responses(
        (status = 200, description = "Machine-readable export of the user's data", body = AccountExportResponse),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "User not found")
    )
)]
/// # Errors
///
/// Returns an error if the user is not found or a repository call fails.
#[tracing::instrument(name = "handler.export_user_data", skip_all)]
pub async fn export_data(
    State(deps): State<Arc<ProfileHandlerDeps>>,
    auth_user: AuthUser,
) -> Result<Response, AppError> {
    // The audit event is recorded before any byte is written; a failure here
    // refuses the export rather than handing data over unrecorded.
    let (user, oauth, lists) = deps
        .export_user_data_use_case
        .execute(&auth_user.user_id)
        .await?;

    let mut preamble = serde_json::to_vec(&build_export_header(user, &oauth))
        .map_err(|e| AppError::Internal(format!("export serialization failed: {e}")))?;
    // `lists` is the DTO's last field. Leave its array open for the stream below.
    if !preamble.ends_with(b"\"lists\":[]}") {
        return Err(AppError::Internal(
            "export layout changed: lists must be the last field".into(),
        ));
    }
    preamble.truncate(preamble.len() - 2);

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    let use_case = deps.export_user_data_use_case.clone();
    let user_id = auth_user.user_id;
    // The stream outlives the handler: keep its logs under the request's trace.
    let stream = async move {
        if tx.send(Ok(Bytes::from(preamble))).await.is_err() {
            return;
        }
        for (index, summary) in lists.into_iter().enumerate() {
            let list_id = *summary.list.id();
            let todos = match use_case.list_todos(&user_id, &list_id).await {
                Ok(todos) => todos,
                Err(error) => {
                    // Abort the response: the browser sees a failed download,
                    // never a silently partial file. No content is logged.
                    tracing::error!(user_id = %user_id, list_id = %list_id, error = %error, "export failed mid-stream");
                    let _ = tx.send(Err(std::io::Error::other("export failed"))).await;
                    return;
                }
            };
            let export = ListExport {
                id: list_id.as_uuid().to_owned(),
                name: summary.list.name().as_str().to_string(),
                version: summary.list.version(),
                created_at: summary.list.created_at(),
                todos: todos
                    .iter()
                    .map(|todo| TodoExport {
                        id: *todo.id().as_uuid(),
                        title: todo.title().as_str().to_string(),
                        version: todo.version(),
                        done: todo.is_done(),
                        created_at: todo.created_at(),
                        completed_at: todo.completed_at(),
                    })
                    .collect(),
            };
            let mut chunk = Vec::new();
            if index > 0 {
                chunk.push(b',');
            }
            if let Err(error) = serde_json::to_writer(&mut chunk, &export) {
                tracing::error!(user_id = %user_id, error = %error, "export serialization failed mid-stream");
                let _ = tx.send(Err(std::io::Error::other("export failed"))).await;
                return;
            }
            if tx.send(Ok(Bytes::from(chunk))).await.is_err() {
                return;
            }
        }
        let _ = tx.send(Ok(Bytes::from_static(b"]}"))).await;
    };
    tokio::spawn(stream.instrument(tracing::Span::current()));

    let mut response = Response::new(Body::from_stream(ReceiverStream::new(rx)));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(response)
}

/// The signed-in Member's own Security activity, newest first (OT-008).
///
/// Only events about the caller's account; who acted is reduced to `you`,
/// `system` or `administrator`, never naming a staff account.
#[utoipa::path(
    get,
    path = "/me/security-activity",
    tag = "Profile",
    params(
        ("limit" = Option<i64>, Query, description = "Max events (default 20, max 50)"),
        ("before_id" = Option<i64>, Query, description = "Only events older than this one (the previous page's next_before_id)")
    ),
    responses(
        (status = 200, description = "A page of the caller's security activity", body = SecurityActivityResponse),
        (status = 401, description = "Not authenticated")
    )
)]
/// # Errors
/// Returns an error if the audit log is unreachable.
#[tracing::instrument(name = "handler.security_activity", skip_all)]
pub async fn security_activity(
    State(deps): State<Arc<ProfileHandlerDeps>>,
    auth_user: AuthUser,
    ApiQuery(query): ApiQuery<SecurityActivityQuery>,
) -> Result<Json<SecurityActivityResponse>, AppError> {
    let page = deps
        .list_security_activity_use_case
        .execute(&auth_user.user_id, query.limit(), query.before_id)
        .await?;
    Ok(Json(page.into()))
}

/// Request account deletion (soft, 14-day grace period).
#[utoipa::path(
    delete,
    path = "/me",
    tag = "Profile",
    request_body = RequestDeletionRequest,
    responses(
        (status = 200, description = "Deletion scheduled", body = AccountDeletionResponse),
        (status = 400, description = "Missing confirmation or password"),
        (status = 401, description = "Not authenticated or wrong password"),
        (status = 404, description = "User not found")
    )
)]
/// # Errors
///
/// Returns an error if confirmation is missing, the password is wrong, or the
/// user is not found.
#[tracing::instrument(name = "handler.request_account_deletion", skip_all)]
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
    path = "/me/cancel-deletion",
    tag = "Profile",
    responses(
        (status = 200, description = "Deletion cancelled", body = AccountDeletionResponse),
        (status = 400, description = "Account already deleted"),
        (status = 401, description = "Not authenticated"),
        (status = 404, description = "User not found")
    )
)]
/// # Errors
///
/// Returns an error if the account is already deleted or the user is not found.
#[tracing::instrument(name = "handler.cancel_account_deletion", skip_all)]
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
