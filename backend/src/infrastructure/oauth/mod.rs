//! OAuth provider client implementations.
//!
//! - [`GoogleOAuthClient`] - Google OAuth 2.0 authorization code flow

mod google;

pub use google::GoogleOAuthClient;
