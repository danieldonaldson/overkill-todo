//! Authentication use cases.
//!
//! ## User Registration (Email-First Flow)
//! - [`RegisterUserUseCase`] - Initiates signup (email only, no password)
//! - [`SendVerificationCodeUseCase`] - Sends a 6-digit verification code
//! - [`ResendVerificationUseCase`] - Resends verification code (rate limited)
//! - [`VerifyEmailUseCase`] - Validates code and issues signup token
//! - [`VerifyMagicLinkUseCase`] - Validates magic link token (backup verification)
//! - [`CompleteSignupUseCase`] - Sets password using signup token
//!
//! ## User Login
//! - [`LoginUserUseCase`] - Retrieves user for authentication
//! - [`GetCurrentUserUseCase`] - Retrieves current authenticated user
//!
//! ## Token Management
//! - [`RefreshTokenUseCase`] - Refreshes JWT using a refresh token
//! - [`RevokeTokenUseCase`] - Revokes a refresh token (logout)
//!
//! ## Password Reset
//! - [`RequestPasswordResetUseCase`] - Sends a 6-digit reset code
//! - [`ValidateResetCodeUseCase`] - Validates code and returns reset token
//! - [`ResetPasswordUseCase`] - Resets password using reset token
//!
//! ## OAuth
//! - [`OAuthCallbackUseCase`] - Handles OAuth provider responses (login/link)
//! - [`OAuthUnlinkUseCase`] - Removes OAuth provider from account

mod complete_signup;
mod get_current_user;
mod login_user;
mod oauth_callback;
mod oauth_unlink;
mod refresh_token;
mod register_user;
mod request_password_reset;
mod resend_verification;
mod reset_password;
mod revoke_token;
mod send_verification_code;
mod validate_reset_code;
mod verify_email;
mod verify_magic_link;

pub use complete_signup::CompleteSignupUseCase;
pub use get_current_user::GetCurrentUserUseCase;
pub use login_user::LoginUserUseCase;
pub use oauth_callback::{OAuthCallbackMode, OAuthCallbackResult, OAuthCallbackUseCase};
pub use oauth_unlink::OAuthUnlinkUseCase;
pub use refresh_token::{RefreshResult, RefreshTokenUseCase};
pub use register_user::RegisterUserUseCase;
pub use request_password_reset::RequestPasswordResetUseCase;
pub use resend_verification::{ResendResult, ResendVerificationError, ResendVerificationUseCase};
pub use reset_password::ResetPasswordUseCase;
pub use revoke_token::RevokeTokenUseCase;
pub use send_verification_code::SendVerificationCodeUseCase;
pub use validate_reset_code::ValidateResetCodeUseCase;
pub use verify_email::{VerifyEmailError, VerifyEmailUseCase};
pub use verify_magic_link::{VerifyMagicLinkError, VerifyMagicLinkUseCase};
