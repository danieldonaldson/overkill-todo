//! Router state.
//!
//! The whole route tree shares one [`AppState`]. Handlers keep extracting the
//! narrow piece they need (`State<Arc<AuthHandlerDeps>>`, `State<Arc<dyn ReadinessCheck>>`, …)
//! through the [`FromRef`] impls below, so no handler signature depends on it.
//!
//! One state type is what lets [`super::routes::api_routes`] declare every route
//! without dependency values: the values are bound once, in
//! [`super::routes::create_router`], and the `OpenAPI` description can be built
//! from the same declarations without a database or any adapter.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::extract::FromRef;

use platform::persistence::{DatabaseError, DatabaseHealth};

use super::handlers::{
    admin_handler::AdminHandlerDeps, auth_handler::AuthHandlerDeps,
    client_telemetry_handler::ClientTelemetryHandlerDeps, oauth_handler::OAuthHandlerDeps,
    profile_handler::ProfileHandlerDeps, verification_handler::VerificationHandlerDeps,
};

/// Object-safe database readiness probe used by `/health/ready`, keeping the
/// HTTP adapter decoupled from any concrete database driver.
pub trait ReadinessCheck: Send + Sync {
    /// Checks whether the backing store is reachable.
    fn ping(&self) -> Pin<Box<dyn Future<Output = Result<(), DatabaseError>> + Send + '_>>;
}

impl<T: DatabaseHealth + Send + Sync> ReadinessCheck for T {
    fn ping(&self) -> Pin<Box<dyn Future<Output = Result<(), DatabaseError>> + Send + '_>> {
        Box::pin(DatabaseHealth::ping(self))
    }
}

/// Every handler group's dependencies, bound to the router once.
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthHandlerDeps>,
    pub verification: Arc<VerificationHandlerDeps>,
    pub oauth: Arc<OAuthHandlerDeps>,
    pub profile: Arc<ProfileHandlerDeps>,
    pub admin: Arc<AdminHandlerDeps>,
    pub client_telemetry: Arc<ClientTelemetryHandlerDeps>,
    pub db: Arc<dyn ReadinessCheck>,
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
    client_telemetry: Arc<ClientTelemetryHandlerDeps>,
    db: Arc<dyn ReadinessCheck>,
}
