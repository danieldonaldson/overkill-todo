//! Shared, dependency-light utilities.
//!
//! - [`crypto`] - Password hashing and JWT key management
//! - [`fingerprint`] - Error fingerprinting for grouping similar errors
//!
//! The HTTP-facing `AppError` and the helpers that produce it (CSRF, JWT, and
//! OAuth-state) live in `infrastructure::api`, so this module stays a leaf.

pub mod crypto;
pub mod fingerprint;

pub use crypto::{Crypto, generate_secure_token, sha256_hex};
