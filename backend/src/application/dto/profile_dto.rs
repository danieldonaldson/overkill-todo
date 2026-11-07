//! Profile-related DTOs for user account management.

use super::validation::validate_verification_code;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

/// Request to update user profile (name).
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateProfileRequest {
    /// New first name (if provided). Must be 1-100 characters if set.
    #[validate(length(min = 1, max = 100, message = "First name must be 1-100 characters"))]
    pub first_name: Option<String>,

    /// New last name (if provided). Must be 1-100 characters if set.
    #[validate(length(min = 1, max = 100, message = "Last name must be 1-100 characters"))]
    pub last_name: Option<String>,
}

/// Response after updating profile.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileResponse {
    pub first_name: String,
    pub last_name: String,
}

/// Request to change password (for users who already have a password).
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ChangePasswordRequest {
    /// Current password for verification.
    #[validate(length(max = 256, message = "Password too long"))]
    pub current_password: String,

    /// New password (8-256 characters).
    #[validate(length(min = 8, max = 256, message = "Password must be 8-256 characters"))]
    pub new_password: String,
}

/// Request to set password (for OAuth-only users without a password).
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetPasswordRequest {
    /// New password (8-256 characters).
    #[validate(length(min = 8, max = 256, message = "Password must be 8-256 characters"))]
    pub new_password: String,
}

/// Request to change email address.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RequestEmailChangeRequest {
    /// Current password for verification.
    #[validate(length(max = 256, message = "Password too long"))]
    pub current_password: String,

    /// New email address.
    #[validate(email(message = "Invalid email format"))]
    pub new_email: String,
}

/// Request to confirm email change with verification code.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ConfirmEmailChangeRequest {
    /// 6-digit verification code sent to new email.
    #[validate(custom(function = "validate_verification_code"))]
    pub code: String,
}

/// Request to delete account.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RequestDeletionRequest {
    /// Password for re-authentication (required for password users).
    #[validate(length(max = 256, message = "Password too long"))]
    pub password: Option<String>,

    /// Confirmation text - must be "DELETE" to proceed.
    pub confirmation: Option<String>,
}

/// A linked OAuth provider account, included in the data export.
#[derive(Debug, Serialize, ToSchema)]
pub struct LinkedOAuthAccountExport {
    /// Provider name (e.g. `"google"`).
    pub provider: String,
    /// Email the provider reported at link time, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_email: Option<String>,
    /// When the provider account was linked.
    pub linked_at: DateTime<Utc>,
}

/// Machine-readable export of the personal data held about the authenticated
/// user (GDPR data-portability). Returned by
/// `GET /api/v1/users/me/export`.
#[derive(Debug, Serialize, ToSchema)]
pub struct AccountExportResponse {
    pub id: Uuid,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub email_verified: bool,
    /// Whether a password is set (the hash itself is never exported).
    pub has_password: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_scheduled_at: Option<DateTime<Utc>>,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// All OAuth providers linked to this account.
    pub linked_oauth_accounts: Vec<LinkedOAuthAccountExport>,
    /// When this export was generated.
    pub exported_at: DateTime<Utc>,
}

/// Response to an account-deletion request or its cancellation.
#[derive(Debug, Serialize, ToSchema)]
pub struct AccountDeletionResponse {
    /// When the account will be permanently anonymized. `null` after a
    /// cancellation; set ~14 days out after a deletion request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_scheduled_at: Option<DateTime<Utc>>,
}
