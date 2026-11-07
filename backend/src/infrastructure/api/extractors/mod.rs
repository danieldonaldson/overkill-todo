//! Axum extractors.

mod admin;
mod authorized;

pub use admin::{AdminUser, SuperAdminUser};
pub use authorized::ValidatedJson;
