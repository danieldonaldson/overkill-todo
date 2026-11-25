//! Value objects for the auth domain.

#[macro_use]
pub(crate) mod id_macros;

// UUID ID types
id_macros::uuid_id!(UserId);
id_macros::uuid_id!(RefreshTokenId);
id_macros::uuid_id!(VerificationCodeId);
id_macros::uuid_id!(ResetCodeId);
id_macros::uuid_id!(ListId);
id_macros::uuid_id!(TodoId);

pub mod account_status;
pub mod email;

pub mod list_name;
pub mod single_line;
pub mod user_role;

pub use account_status::{AccountStatus, AlreadyDeleted};
pub use email::Email;

pub use list_name::{ListName, MAX_LIST_NAME_CHARS, MAX_TODO_TITLE_CHARS, TodoTitle};
pub use user_role::{RoleChangeRefusal, UserRole};
