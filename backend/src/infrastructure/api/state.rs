//! Router state.
//!
//! The whole route tree shares one [`AppState`]. Handlers keep extracting the
//! narrow piece they need (`State<Arc<AuthHandlerDeps>>`, `State<Arc<Database>>`, …)
//! through the [`FromRef`] impls below, so no handler signature depends on it.
//!
//! One state type is what lets [`super::routes::api_routes`] declare every route
//! without dependency values: the values are bound once, in
//! [`super::routes::create_router`], and the `OpenAPI` description can be built
//! from the same declarations without a database or any adapter.

use std::sync::Arc;

use axum::extract::FromRef;

use crate::infrastructure::persistence::Database;

use super::handlers::{
    admin_handler::AdminHandlerDeps, auth_handler::AuthHandlerDeps,
    oauth_handler::OAuthHandlerDeps, profile_handler::ProfileHandlerDeps,
    verification_handler::VerificationHandlerDeps,
};

/// Every handler group's dependencies, bound to the router once.
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthHandlerDeps>,
    pub verification: Arc<VerificationHandlerDeps>,
    pub oauth: Arc<OAuthHandlerDeps>,
    pub profile: Arc<ProfileHandlerDeps>,
    pub admin: Arc<AdminHandlerDeps>,
    pub db: Arc<Database>,
}

macro_rules! from_ref {
    ($($field:ident: $ty:ty),* $(,)?) => {
        $(
            impl FromRef<AppState> for $ty {
                fn from_ref(state: &AppState) -> Self {
                    state.$field.clone()
                }
            }
        )*
    };
}

from_ref! {
    auth: Arc<AuthHandlerDeps>,
    verification: Arc<VerificationHandlerDeps>,
    oauth: Arc<OAuthHandlerDeps>,
    profile: Arc<ProfileHandlerDeps>,
    admin: Arc<AdminHandlerDeps>,
    db: Arc<Database>,
}
