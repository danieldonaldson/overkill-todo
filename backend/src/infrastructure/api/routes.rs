//! HTTP router: route table, per-group middleware, and global layers.
//!
//! Every documented route is mounted with `routes!(handler)` in [`api_routes`],
//! which takes the method and path from the handler's `#[utoipa::path]`. The
//! router and the `OpenAPI` description are therefore built from the same
//! declarations and cannot drift. Annotation paths are relative to the group
//! they are nested in (`path = "/login"` under `/auth` under `/api/v1`).

use axum::{
    Extension, Router,
    http::{HeaderName, HeaderValue, Method},
    middleware,
    routing::get,
};
use std::sync::Arc;
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};
#[cfg(feature = "swagger-ui")]
use utoipa_swagger_ui::SwaggerUi;

use super::openapi::ApiDoc;
#[cfg(feature = "swagger-ui")]
use crate::infrastructure::config::is_dev_mode;
use crate::infrastructure::crypto::Crypto;
use crate::infrastructure::persistence::Database;
use crate::infrastructure::telemetry::{
    http::{http_trace_layer, record_request_metrics},
    metrics::HttpMetrics,
};

use super::{
    handlers::{
        admin_handler::{self, AdminHandlerDeps},
        auth_handler::{self, AuthHandlerDeps},
        health_handler,
        oauth_handler::{self, OAuthHandlerDeps},
        profile_handler::{self, ProfileHandlerDeps},
        verification_handler::{self, VerificationHandlerDeps},
    },
    middleware::csrf::is_excluded_path,
    middleware::{
        auth_middleware, csrf_middleware, require_fresh_session, require_verified_email,
        security_headers_middleware, trace_id_middleware,
    },
    openapi::{add_response_if_missing, for_each_operation},
    state::AppState,
};
use crate::application::ports::outgoing::UserRepository;

/// Request timeout for every route. Keep it above any outbound call a handler
/// makes inline, or the client sees 408 after the work already committed.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// All handler dependencies required to build the router.
pub struct RouterDeps {
    pub auth: Arc<AuthHandlerDeps>,
    pub verification: Arc<VerificationHandlerDeps>,
    pub oauth: Arc<OAuthHandlerDeps>,
    pub profile: Arc<ProfileHandlerDeps>,
    pub admin: Arc<AdminHandlerDeps>,
    /// Database handle for the `/health/ready` probe.
    pub db: Arc<Database>,
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

/// Route-group guards. Each applies a middleware `route_layer` *and* documents
/// the status that middleware can return on every operation it covers, so the
/// description can't miss a protection or describe one that isn't there. Call
/// them in the same order the `route_layer`s were applied before (the last call
/// runs first).
trait Guarded: Sized {
    /// `auth_middleware`: rejects a missing, invalid or expired session.
    fn require_auth(self) -> Self;
    /// `require_verified_email`: rejects an account whose email isn't verified.
    fn require_verified_email(self) -> Self;
    /// `require_fresh_session`: rejects a token issued before a password reset,
    /// account purge or role change.
    fn require_fresh_session(self) -> Self;
    /// `csrf_middleware`: rejects state-changing requests without a valid
    /// CSRF token, except on the paths it excludes.
    fn require_csrf(self) -> Self;
}

fn document(
    mut router: OpenApiRouter<AppState>,
    applies: impl Fn(&str, &str) -> bool,
    status: &str,
    description: &str,
) -> OpenApiRouter<AppState> {
    for_each_operation(router.get_openapi_mut(), |method, path, op| {
        if applies(method, path) {
            add_response_if_missing(op, status, description);
        }
    });
    router
}

impl Guarded for OpenApiRouter<AppState> {
    fn require_auth(self) -> Self {
        document(
            self.route_layer(middleware::from_fn(auth_middleware)),
            |_, _| true,
            "401",
            "Not signed in, or the session has expired",
        )
    }

    fn require_verified_email(self) -> Self {
        document(
            self.route_layer(middleware::from_fn(require_verified_email)),
            |_, _| true,
            "403",
            "Email address not verified (`email_not_verified`)",
        )
    }

    fn require_fresh_session(self) -> Self {
        document(
            self.route_layer(middleware::from_fn(require_fresh_session)),
            |_, _| true,
            "401",
            "Session superseded by a password reset, account purge or role change; sign in again",
        )
    }

    fn require_csrf(self) -> Self {
        document(
            self.route_layer(middleware::from_fn(csrf_middleware)),
            |method, path| {
                matches!(method, "POST" | "PUT" | "PATCH" | "DELETE") && !is_excluded_path(path)
            },
            "403",
            "Missing or invalid CSRF token (`csrf_invalid`)",
        )
    }
}

/// Every documented route, its group middleware and the `/api/v1` prefix.
///
/// Takes no dependency values: state is bound later by [`create_router`], and
/// [`super::openapi::openapi`] builds the description from this alone. Use one
/// `routes!` per path: the macro mounts all of its handlers on every path it sees.
pub fn api_routes() -> OpenApiRouter<AppState> {
    // Public auth routes (no auth required)
    let public_auth = OpenApiRouter::new()
        .routes(routes!(auth_handler::register))
        .routes(routes!(auth_handler::login))
        .routes(routes!(auth_handler::logout))
        .routes(routes!(auth_handler::refresh));

    // Authenticated auth routes
    let private_auth = OpenApiRouter::new()
        .routes(routes!(auth_handler::me))
        .require_auth();

    // Verification routes (no auth)
    let verification = OpenApiRouter::new()
        .routes(routes!(verification_handler::verify_email))
        .routes(routes!(
            verification_handler::verify_magic_redirect,
            verification_handler::verify_magic
        ))
        .routes(routes!(verification_handler::resend_verification))
        .routes(routes!(verification_handler::forgot_password))
        .routes(routes!(verification_handler::verify_reset_code))
        .routes(routes!(verification_handler::reset_password))
        .routes(routes!(verification_handler::complete_signup));

    // OAuth routes
    let oauth = OpenApiRouter::new()
        .routes(routes!(oauth_handler::oauth_store_device_fingerprint))
        .routes(routes!(oauth_handler::oauth_start))
        .routes(routes!(oauth_handler::oauth_callback));

    let oauth_protected = OpenApiRouter::new()
        .routes(routes!(oauth_handler::oauth_unlink))
        .require_auth();

    // Profile routes (require auth + verified email)
    let profile = OpenApiRouter::new()
        .routes(routes!(profile_handler::change_password))
        .routes(routes!(profile_handler::set_password))
        .routes(routes!(profile_handler::export_data))
        .routes(routes!(profile_handler::request_deletion))
        .routes(routes!(profile_handler::cancel_deletion))
        // Account-sensitive actions (password, export, deletion): reject
        // tokens issued before a password reset, account purge or role change.
        // One extra DB read on these low-traffic routes only; ordinary routes
        // trust the token until it expires (JWT_EXPIRATION_MINUTES). Innermost,
        // so the free verified-email check rejects first.
        .require_fresh_session()
        .require_verified_email()
        .require_auth();

    // Admin routes (require auth + verified email; the `AdminUser` /
    // `SuperAdminUser` extractors on each handler enforce the role). The first
    // admin is bootstrapped out-of-band with `just grant-admin <email>`.
    let admin = OpenApiRouter::new()
        .routes(routes!(admin_handler::list_users))
        .routes(routes!(admin_handler::set_user_role))
        .require_verified_email()
        // Runs after `auth_middleware` (AuthUser populated), before the handler:
        // reject tokens superseded by a role change so demotions take effect at
        // once. Admin-only, so ordinary traffic pays no extra DB read. The user
        // repository comes from the `Extension` installed in `create_router`.
        .require_fresh_session()
        .require_auth();

    // Health probes, reachable through the single-origin `/api` route for
    // external monitoring. Public (no auth/CSRF), so merged after
    // `require_csrf()` below. The root-level copy for the container HEALTHCHECK is
    // mounted in `create_router`.
    let health = OpenApiRouter::new()
        .routes(routes!(health_handler::health_check))
        .routes(routes!(health_handler::health_ready));

    let api = OpenApiRouter::new()
        .nest("/auth", public_auth)
        .nest("/auth", private_auth)
        .nest("/auth", verification)
        .nest("/auth", oauth)
        .nest("/auth", oauth_protected)
        .nest("/users", profile)
        .nest("/admin", admin)
        // Mount new authenticated groups above this line so they get CSRF.
        // Public, signature-verified endpoints (e.g. payment webhooks) go below it.
        .require_csrf()
        .merge(health)
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            DEFAULT_REQUEST_TIMEOUT,
        ));

    OpenApiRouter::with_openapi(ApiDoc::openapi()).nest("/api/v1", api)
}

/// Build the application router.
///
/// # Panics
///
/// Panics if `frontend_origin` is not a valid `HeaderValue`.
pub fn create_router(deps: RouterDeps, frontend_origin: &str) -> Router {
    let cors = build_cors(frontend_origin);
    let (api, spec) = api_routes().split_for_parts();

    // Root-level health probes for the in-container HEALTHCHECK, which hits the
    // loopback port and bypasses the proxy (the proxy sends `/` to the
    // frontend). Same handlers as the documented `/api/v1/health*`.
    #[expect(
        clippy::disallowed_methods,
        reason = "undocumented duplicate of the described /api/v1/health* for the container \
                  HEALTHCHECK; describing both would duplicate operation IDs"
    )]
    let root_health = Router::new()
        .route("/health", get(health_handler::health_check))
        .route(
            "/health/ready",
            get(health_handler::health_ready).with_state(deps.db.clone()),
        );

    let state = AppState {
        auth: deps.auth,
        verification: deps.verification,
        oauth: deps.oauth,
        profile: deps.profile,
        admin: deps.admin,
        db: deps.db,
    };

    let router = Router::new()
        .merge(root_health)
        .merge(api.with_state(state));

    // Swagger UI exposes the full API surface; only mount it in dev mode. Release images
    // build without the `swagger-ui` feature, so the bundle isn't even in the binary.
    #[cfg(feature = "swagger-ui")]
    let router = if is_dev_mode() {
        router.merge(
            SwaggerUi::new("/docs").url("/api-doc/openapi.json", super::openapi::finalize(spec)),
        )
    } else {
        router
    };
    #[cfg(not(feature = "swagger-ui"))]
    let _ = spec;

    router
        .layer(middleware::from_fn(security_headers_middleware))
        .layer(cors)
        // Make the crypto handle and the user repository available as request
        // extensions to every inner middleware/extractor (auth, CSRF,
        // OptionalAuthUser, require_fresh_session).
        .layer(Extension(deps.crypto))
        .layer(Extension(deps.users))
        // Stamp every response (including ones produced by inner middleware)
        // with the request's `X-Trace-Id`, read from the span opened below.
        .layer(middleware::from_fn(trace_id_middleware))
        // Request duration per route template (`http.server.request.duration`),
        // measured inside the span so it covers every inner layer.
        .layer(middleware::from_fn_with_state(
            HttpMetrics::from_global(),
            record_request_metrics,
        ))
        // Outermost: one server span per request. `Router::layer` runs after
        // routing, so the span is named after the matched route template.
        .layer(http_trace_layer())
}
