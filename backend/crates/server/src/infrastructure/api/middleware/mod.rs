//! Axum middleware.

pub mod auth;
pub mod csrf;
pub mod security_headers;
pub mod trace_id;
pub mod verification;

pub use auth::{AuthUser, OptionalAuthUser, auth_middleware, require_fresh_session};
pub use csrf::csrf_middleware;
pub use security_headers::security_headers_middleware;
pub use trace_id::trace_id_middleware;
pub use verification::require_verified_email;
