//! Application use cases (auth slice).

pub mod admin;

pub mod auth;

pub mod user;

pub use admin::{ListUsersUseCase, SetUserRoleUseCase};

pub use auth::{
    CompleteSignupUseCase, GetCurrentUserUseCase, LoginError, LoginUserUseCase, OAuthCallbackMode,
    OAuthCallbackResult, OAuthCallbackUseCase, OAuthUnlinkUseCase, RefreshResult,
    RefreshTokenUseCase, RegisterOutcome, RegisterUserUseCase, RequestPasswordResetUseCase,
    ResendResult, ResendVerificationError, ResendVerificationUseCase, ResetPasswordUseCase,
    RevokeTokenUseCase, SendVerificationCodeUseCase, ValidateResetCodeUseCase, VerifyEmailError,
    VerifyEmailUseCase, VerifyMagicLinkError, VerifyMagicLinkUseCase,
};

pub use user::{
    CancelAccountDeletionUseCase, ChangePasswordUseCase, ExportUserDataUseCase,
    PurgeScheduledAccountsUseCase, RequestAccountDeletionUseCase, SetPasswordUseCase,
};
