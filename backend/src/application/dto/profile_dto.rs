//! Profile-related DTOs for user account management.

use super::schema_enums::{OAuthProviderName, RoleName};
use super::validation::validate_verification_code;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

/// Request to update user profile (name).
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateProfileRequest {
    /// New first name (if provided). Must be 1-100 characters if set.
    #[schema(min_length = 1, max_length = 100)]
    #[validate(length(min = 1, max = 100, message = "First name must be 1-100 characters"))]
    #[serde(default)]
    pub first_name: Option<String>,

    /// New last name (if provided). Must be 1-100 characters if set.
    #[schema(min_length = 1, max_length = 100)]
    #[validate(length(min = 1, max = 100, message = "Last name must be 1-100 characters"))]
    #[serde(default)]
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
    #[schema(max_length = 256)]
    #[validate(length(max = 256, message = "Password too long"))]
    pub current_password: String,

    /// New password (8-256 characters).
    #[schema(min_length = 8, max_length = 256)]
    #[validate(length(min = 8, max = 256, message = "Password must be 8-256 characters"))]
    pub new_password: String,
}

/// Request to set password (for OAuth-only users without a password).
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetPasswordRequest {
    /// New password (8-256 characters).
    #[schema(min_length = 8, max_length = 256)]
    #[validate(length(min = 8, max = 256, message = "Password must be 8-256 characters"))]
    pub new_password: String,
}

/// Request to change email address.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RequestEmailChangeRequest {
    /// Current password for verification.
    #[schema(max_length = 256)]
    #[validate(length(max = 256, message = "Password too long"))]
    pub current_password: String,

    /// New email address.
    #[schema(format = Email)]
    #[validate(email(message = "Invalid email format"))]
    pub new_email: String,
}

/// Request to confirm email change with verification code.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ConfirmEmailChangeRequest {
    /// 6-digit verification code sent to new email.
    // Exactly 6 ASCII digits once separators are stripped (validation.rs).
    #[schema(max_length = 32, pattern = "^[^0-9]*([0-9][^0-9]*){6}$")] // MAX_FORMATTED_CODE_LEN
    #[validate(custom(function = "validate_verification_code"))]
    pub code: String,
}

/// Request to delete account.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RequestDeletionRequest {
    /// Password for re-authentication (required for password users).
    #[schema(max_length = 256)]
    #[validate(length(max = 256, message = "Password too long"))]
    #[serde(default)]
    pub password: Option<String>,

    /// Confirmation text - must be "DELETE" to proceed. Required: a request
    /// without it is refused with 400 (deserialised as optional only so that
    /// refusal carries the specific message).
    #[schema(required = true, value_type = String, pattern = "^DELETE$")]
    #[serde(default)]
    pub confirmation: Option<String>,
}

/// A linked OAuth provider account, included in the data export.
#[derive(Debug, Serialize, ToSchema)]
pub struct LinkedOAuthAccountExport {
    /// Provider name (e.g. `"google"`).
    #[schema(value_type = OAuthProviderName)]
    pub provider: String,
    /// Email the provider reported at link time, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub provider_email: Option<String>,
    /// When the provider account was linked.
    pub linked_at: Timestamp,
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
    #[schema(nullable = false)]
    pub pending_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub deletion_scheduled_at: Option<Timestamp>,
    #[schema(value_type = RoleName)]
    pub role: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// All OAuth providers linked to this account.
    pub linked_oauth_accounts: Vec<LinkedOAuthAccountExport>,
    /// When this export was generated.
    pub exported_at: Timestamp,
}

/// Response to an account-deletion request or its cancellation.
#[derive(Debug, Serialize, ToSchema)]
pub struct AccountDeletionResponse {
    /// When the account will be permanently anonymized. `null` after a
    /// cancellation; set ~14 days out after a deletion request.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub deletion_scheduled_at: Option<Timestamp>,
}
