//! Auth API handlers.

pub mod admin_handler;

pub mod auth_handler;

pub mod health_handler;
pub mod oauth_handler;
pub mod profile_handler;
pub mod session;
pub mod verification_handler;

use crate::application::ports::TurnstileService;
use crate::infrastructure::api::error::AppError;

/// Verify a Turnstile token for an incoming request.
///
/// When bot protection is enabled ([`TurnstileService::is_enabled`]), a token is
/// **required** — a missing or empty token is rejected. This closes the bypass
/// where a client could skip verification simply by omitting the token. When
/// protection is disabled (local development), verification is a no-op.
///
/// # Errors
///
/// Returns [`AppError::Validation`] if the token is missing/empty (while enabled)
/// or fails verification, and [`AppError::Internal`] if the upstream call errors.
pub async fn verify_turnstile(
    service: &dyn TurnstileService,
    token: Option<&str>,
) -> Result<(), AppError> {
    if !service.is_enabled() {
        return Ok(());
    }

    let token = token
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::Validation("Bot verification required.".into()))?;

    let is_valid = service
        .verify(token)
        .await
        .map_err(|e| AppError::Internal(format!("Turnstile verification error: {e}")))?;

    if !is_valid {
        return Err(AppError::Validation(
            "Bot verification failed. Please try again.".into(),
        ));
    }

    Ok(())
}
