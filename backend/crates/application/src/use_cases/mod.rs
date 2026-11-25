//! Application use cases (auth slice).

pub mod admin;

pub mod auth;

pub mod lists;
pub mod telemetry;

pub mod user;

pub use admin::{ListAuditEventsUseCase, ListUsersUseCase, SetUserRoleUseCase};

pub use auth::{
    CompleteSignupUseCase, GetCurrentUserUseCase, LoginError, LoginUserUseCase, OAuthCallbackMode,
    OAuthCallbackResult, OAuthCallbackUseCase, OAuthUnlinkUseCase, RefreshError, RefreshResult,
    RefreshTokenUseCase, RegisterOutcome, RegisterUserUseCase, RequestPasswordResetUseCase,
    ResendResult, ResendVerificationError, ResendVerificationUseCase, ResetCodeError,
    ResetPasswordUseCase, RevokeTokenUseCase, SendVerificationCodeUseCase,
    ValidateResetCodeUseCase, VerifyEmailError, VerifyEmailUseCase, VerifyMagicLinkError,
    VerifyMagicLinkUseCase,
};

pub use lists::{
    AddTodoUseCase, CreateListUseCase, DeleteListUseCase, DeleteTodoUseCase, GetListUseCase,
    ListError, ListListsUseCase, MarkTodoUseCase, MutationOutcome, RenameListUseCase,
    RenameTodoUseCase,
};

pub use telemetry::RecordClientTelemetryUseCase;

pub use user::{
    CancelAccountDeletionUseCase, ChangePasswordUseCase, ExportUserDataUseCase,
    ListSecurityActivityUseCase, PurgeScheduledAccountsUseCase, RequestAccountDeletionUseCase,
    SetPasswordUseCase,
};
