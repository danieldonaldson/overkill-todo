//! Value objects for the auth domain.

#[macro_use]
pub(crate) mod id_macros;

// UUID ID types
id_macros::uuid_id!(UserId, serde);
id_macros::uuid_id!(RefreshTokenId);
id_macros::uuid_id!(VerificationCodeId);
id_macros::uuid_id!(ResetCodeId);

pub mod account_status;
pub mod email;
pub mod pending_email_change;

pub mod user_role;

pub use account_status::AccountStatus;
pub use email::Email;
pub use pending_email_change::PendingEmailChange;

pub use user_role::UserRole;
