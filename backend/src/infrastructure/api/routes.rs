//! HTTP router: route table, per-group middleware, and global layers.

use axum::{
    Extension, Router,
    http::{HeaderName, HeaderValue, Method},
    middleware,
    routing::{delete, get, post, put},
};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::infrastructure::config::is_dev_mode;
use crate::shared::crypto::Crypto;

use super::{
    handlers::{
        admin_handler::{self, AdminHandlerDeps},
        auth_handler::{self, AuthHandlerDeps},
        health_handler,
        oauth_handler::{self, OAuthHandlerDeps},
        profile_handler::{self, ProfileHandlerDeps},
        verification_handler::{self, VerificationHandlerDeps},
    },
    middleware::{
        auth_middleware, csrf_middleware, require_fresh_session, require_verified_email,
        security_headers_middleware, trace_id_middleware,
    },
    openapi::ApiDoc,
};
use crate::application::ports::outgoing::UserRepository;

/// Request timeout for every route. Keep it above any outbound call a handler
/// makes inline, or the client sees 408 after the work already committed.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Canonical list of every route template mounted by [`create_router`], using the
/// same `{param}` placeholder syntax as `#[utoipa::path(...)]` declarations.
///
/// This is the single source of truth used by the `OpenAPI` routability test
/// (`openapi.rs`) to guarantee every documented path is actually served. When you
/// add, remove, or rename a `.route(...)` below, update this list to match — the
/// test will fail otherwise.
#[cfg(test)]
pub(crate) const MOUNTED_ROUTE_TEMPLATES: &[&str] = &[
    // Health (mounted at the root, not under /api/v1)
    "/health",
    "/health/ready",
    // Public auth
    "/api/v1/auth/register",
    "/api/v1/auth/login",
    "/api/v1/auth/logout",
    "/api/v1/auth/refresh",
    // Private auth
    "/api/v1/auth/me",
    // Verification
    "/api/v1/auth/verify-email",
    "/api/v1/auth/verify-magic",
    "/api/v1/auth/resend-verification",
    "/api/v1/auth/forgot-password",
    "/api/v1/auth/verify-reset-code",
    "/api/v1/auth/reset-password",
    "/api/v1/auth/complete-signup",
    // OAuth
    "/api/v1/auth/oauth/device-fingerprint",
    "/api/v1/auth/oauth/{provider}",
    "/api/v1/auth/oauth/{provider}/callback",
    "/api/v1/auth/oauth/{provider}/unlink",
    // Profile
    "/api/v1/users/password/change",
    "/api/v1/users/password/set",
    "/api/v1/users/me/export",
    "/api/v1/users/me",
    "/api/v1/users/me/cancel-deletion",
    // Admin
    "/api/v1/admin/users",
    "/api/v1/admin/users/{id}/role",
];

/// All handler dependencies required to build the router.
pub struct RouterDeps {
    pub auth: Arc<AuthHandlerDeps>,
    pub verification: Arc<VerificationHandlerDeps>,
    pub oauth: Arc<OAuthHandlerDeps>,
    pub profile: Arc<ProfileHandlerDeps>,
    pub admin: Arc<AdminHandlerDeps>,
    pub db_pool: PgPool,
    /// User repository handle for the admin-group [`require_fresh_session`]
    /// middleware, which rejects tokens superseded by a privilege change.
    pub users: Arc<dyn UserRepository>,
    /// Crypto handle injected as a request extension so the state-free auth /
    /// CSRF middleware and the `OptionalAuthUser` extractor can verify tokens
    /// without reaching a global singleton.
    pub crypto: Arc<Crypto>,
}

/// Build the CORS layer allowing the configured frontend origin with credentials.
///
/// `frontend_origin` is expected to already be a validated origin (see
/// `Settings::from_env`, which normalizes `FRONTEND_BASE_URL` and rejects a
/// malformed value at startup), so the parse below cannot fail in practice.
///
/// # Panics
///
/// Panics if `frontend_origin` is not a valid `HeaderValue` — unreachable when
/// the value comes from `settings.server.frontend_origin`.
fn build_cors(frontend_origin: &str) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(
            frontend_origin
                .parse::<HeaderValue>()
                .expect("Invalid frontend origin"),
        )
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            HeaderName::from_static("content-type"),
            HeaderName::from_static("authorization"),
            HeaderName::from_static("x-csrf-token"),
            HeaderName::from_static("x-device-fingerprint"),
            HeaderName::from_static("x-request-id"),
        ])
        .allow_credentials(true)
}

/// Build the application router.
///
/// # Panics
///
/// Panics if `frontend_origin` is not a valid `HeaderValue`.
#[allow(clippy::too_many_lines)]
pub fn create_router(deps: RouterDeps, frontend_origin: &str) -> Router {
    let cors = build_cors(frontend_origin);
    let crypto = deps.crypto.clone();
    let users = deps.users.clone();

    // Public auth routes (no auth required)
    let public_auth = Router::new()
        .route("/register", post(auth_handler::register))
        .route("/login", post(auth_handler::login))
        .route("/logout", post(auth_handler::logout))
        .route("/refresh", post(auth_handler::refresh))
        .with_state(deps.auth.clone());

    // Authenticated auth routes
    let private_auth = Router::new()
        .route("/me", get(auth_handler::me))
        .route_layer(middleware::from_fn(auth_middleware))
        .with_state(deps.auth);

    // Verification routes (no auth)
    let verification = Router::new()
        .route("/verify-email", post(verification_handler::verify_email))
        .route("/verify-magic", get(verification_handler::verify_magic))
        .route(
            "/resend-verification",
            post(verification_handler::resend_verification),
        )
        .route(
            "/forgot-password",
            post(verification_handler::forgot_password),
        )
        .route(
            "/verify-reset-code",
            post(verification_handler::verify_reset_code),
        )
        .route(
            "/reset-password",
            post(verification_handler::reset_password),
        )
        .route(
            "/complete-signup",
            post(verification_handler::complete_signup),
        )
        .with_state(deps.verification);

    // OAuth routes
    let oauth = Router::new()
        .route(
            "/oauth/device-fingerprint",
            post(oauth_handler::oauth_store_device_fingerprint),
        )
        .route("/oauth/{provider}", get(oauth_handler::oauth_start))
        .route(
            "/oauth/{provider}/callback",
            get(oauth_handler::oauth_callback),
        )
        .with_state(deps.oauth.clone());

    let oauth_protected = Router::new()
        .route(
            "/oauth/{provider}/unlink",
            delete(oauth_handler::oauth_unlink),
        )
        .route_layer(middleware::from_fn(auth_middleware))
        .with_state(deps.oauth);

    // Profile routes (require auth + verified email)
    let profile = Router::new()
        .route("/password/change", post(profile_handler::change_password))
        .route("/password/set", post(profile_handler::set_password))
        .route("/me/export", get(profile_handler::export_data))
        .route("/me", delete(profile_handler::request_deletion))
        .route(
            "/me/cancel-deletion",
            post(profile_handler::cancel_deletion),
        )
        .route_layer(middleware::from_fn(require_verified_email))
        .route_layer(middleware::from_fn(auth_middleware))
        .with_state(deps.profile);

    // Admin routes (require auth + verified email; the `AdminUser` /
    // `SuperAdminUser` extractors on each handler enforce the role). The first
    // admin is bootstrapped out-of-band with `just grant-admin <email>`.
    let admin = Router::new()
        .route("/users", get(admin_handler::list_users))
        .route("/users/{id}/role", put(admin_handler::set_user_role))
        .route_layer(middleware::from_fn(require_verified_email))
        // Runs after `auth_middleware` (AuthUser populated), before the handler:
        // reject tokens superseded by a role change so demotions take effect at
        // once. Admin-only, so ordinary traffic pays no extra DB read.
        .route_layer(middleware::from_fn_with_state(
            users.clone(),
            require_fresh_session,
        ))
        .route_layer(middleware::from_fn(auth_middleware))
        .with_state(deps.admin);

    // Health probes. Public (no auth/CSRF), so they sit OUTSIDE the CSRF-layered
    // routes below. Mounted in two places: at the root, where the in-container
    // HEALTHCHECK hits them on the loopback port (bypassing the proxy); and under
    // `/api/v1`, so external monitoring reaches them through the single-origin
    // `/api` route (the proxy sends the root `/` to the frontend).
    let health = Router::new()
        .route("/health", get(health_handler::health_check))
        .route(
            "/health/ready",
            get(health_handler::health_ready).with_state(deps.db_pool),
        );

    let api = Router::new()
        .nest("/auth", public_auth)
        .nest("/auth", private_auth)
        .nest("/auth", verification)
        .nest("/auth", oauth)
        .nest("/auth", oauth_protected)
        .nest("/users", profile)
        .nest("/admin", admin)
        // Mount new authenticated groups above this line so they get CSRF.
        // Public, signature-verified endpoints (e.g. payment webhooks) go below it.
        .route_layer(middleware::from_fn(csrf_middleware))
        .merge(health.clone())
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            DEFAULT_REQUEST_TIMEOUT,
        ));

    let mut router = Router::new().merge(health).nest("/api/v1", api);

    // Swagger UI exposes the full API surface; only mount it in dev mode.
    if is_dev_mode() {
        router =
            router.merge(SwaggerUi::new("/docs").url("/api-doc/openapi.json", ApiDoc::openapi()));
    }

    router
        .layer(middleware::from_fn(security_headers_middleware))
        .layer(cors)
        // Make the crypto handle available as a request extension to every inner
        // middleware/extractor (auth, CSRF, OptionalAuthUser).
        .layer(Extension(crypto))
        // Outermost: stamp every response (including ones produced by inner
        // middleware) with an `X-Trace-Id` header for request correlation.
        .layer(middleware::from_fn(trace_id_middleware))
}
