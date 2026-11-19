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
use crate::infrastructure::telemetry::metrics::{AuthMetrics, BotCheckForm, BotCheckReason};

/// Verify a Turnstile token for an incoming request.
///
/// When bot protection is enabled ([`TurnstileService::is_enabled`]), a token is
/// **required** — a missing or empty token is rejected. This closes the bypass
/// where a client could skip verification simply by omitting the token. When
/// protection is disabled (local development), verification is a no-op.
///
/// # Errors
///
/// Returns [`AppError::BotCheckFailed`] (`400 bot_check_failed`) if the token is
/// missing/empty (while enabled) or fails verification, and
/// [`AppError::Internal`] if the upstream call errors.
pub async fn verify_turnstile(
    service: &dyn TurnstileService,
    token: Option<&str>,
    form: BotCheckForm,
    metrics: &AuthMetrics,
) -> Result<(), AppError> {
    if !service.is_enabled() {
        return Ok(());
    }

    let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) else {
        metrics.bot_check_rejection(form, BotCheckReason::Missing);
        return Err(AppError::BotCheckFailed(
            "Bot verification required.".into(),
        ));
    };

    let is_valid = service
        .verify(token)
        .await
        .map_err(|e| AppError::Internal(format!("Turnstile verification error: {e}")))?;

    // An upstream error above is a 500, not a rejection; the 5xx alert covers it.
    if !is_valid {
        metrics.bot_check_rejection(form, BotCheckReason::Invalid);
        return Err(AppError::BotCheckFailed(
            "Bot verification failed. Please try again.".into(),
        ));
    }

    Ok(())
}
