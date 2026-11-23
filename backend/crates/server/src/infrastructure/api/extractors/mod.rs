//! Axum extractors.

mod admin;
mod authorized;
mod rejection;

pub use admin::{AdminUser, SuperAdminUser};
pub use authorized::ValidatedJson;
pub use rejection::{ApiJson, ApiPath, ApiQuery};
