//! Every route is either public by name or guarded (OT-010; threat model P-06,
//! P-07).
//!
//! Walks every operation in the API description, which is built from the same
//! `api_routes()` the server mounts, so a new route is covered here without
//! anyone remembering to add it:
//!
//! - an operation not in [`PUBLIC_ROUTES`] refuses a request with no session,
//!   a garbage token or an expired one;
//! - every `/admin` operation refuses a verified Member with `access_denied`;
//! - every entry in [`PUBLIC_ROUTES`] still exists, so the list can't go stale.
//!
//! Admin routes live under `/api/v1/admin` by convention (ARCHITECTURE.md); an
//! admin-only handler mounted elsewhere is outside this check. Because each
//! admin handler also has a role extractor, the walk alone can't tell whether
//! the *route layer* enforces the role, so `admin_guards_refuse_members_without_
//! any_handler_check` wraps the real guard chains around a handler with no
//! extractor.
//!
//! "Refuses without a session" means 401. A state-changing request may instead
//! get 403 `csrf_invalid`: the CSRF check runs first and itself needs a valid
//! session, so the handler is unreachable either way.

mod common;

use std::sync::Arc;

use application::ports::outgoing::UserRepository;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::{Extension, Router, routing::get};
use domain::value_objects::UserRole;
use persistence_postgres::testing;
use persistence_postgres::{self, Database};
use serde_json::Value;
use server::infrastructure::api::routes::{admin_guards, super_admin_guards};
use sqlx::PgPool;
use tower::ServiceExt;
use utoipa_axum::router::OpenApiRouter;
use uuid::Uuid;

/// Operations reachable without a session, as `"METHOD /path"`. Adding a route
/// here is a security decision: it needs a reason and a threat-model property.
const PUBLIC_ROUTES: &[&str] = &[
    // Sign-in, sign-up and session lifecycle.
    "POST /api/v1/auth/register",
    "POST /api/v1/auth/login",
    "POST /api/v1/auth/logout",
    "POST /api/v1/auth/refresh",
    // Codes, magic links and password reset: the code or link is the credential.
    "POST /api/v1/auth/verify-email",
    "GET /api/v1/auth/verify-magic",
    "POST /api/v1/auth/verify-magic",
    "POST /api/v1/auth/resend-verification",
    "POST /api/v1/auth/forgot-password",
    "POST /api/v1/auth/verify-reset-code",
    "POST /api/v1/auth/reset-password",
    "POST /api/v1/auth/complete-signup",
    // Google sign-in: the signed state is the credential. Starting a *link*
    // needs a session, which the handler checks (oauth_start_link_without_auth_returns_401).
    "POST /api/v1/auth/oauth/device-fingerprint",
    "GET /api/v1/auth/oauth/{provider}",
    "GET /api/v1/auth/oauth/{provider}/callback",
    // Health probes.
    "GET /api/v1/health",
    "GET /api/v1/health/ready",
    // Browser error reports: anyone's browser sends them, signed in or not. The
    // session only attributes a report; the endpoint changes no state and
    // returns nothing (threat model P-34).
    "POST /api/v1/client-telemetry",
];

/// Every `"METHOD /path"` in the API description.
fn operations() -> Vec<(String, String)> {
    let spec = serde_json::to_value(server::infrastructure::api::openapi::openapi())
        .expect("API description serialises");
    let mut ops = Vec::new();
    for (path, item) in spec["paths"].as_object().expect("description has paths") {
        for method in ["get", "post", "put", "patch", "delete"] {
            if item.get(method).is_some() {
                ops.push((method.to_uppercase(), path.clone()));
            }
        }
    }
    assert!(!ops.is_empty(), "the description lists no operations");
    ops
}

/// A concrete URI for a templated path.
fn concrete(path: &str) -> String {
    path.replace("{provider}", "google")
        .replace("{id}", &Uuid::now_v7().to_string())
}

fn is_public(method: &str, path: &str) -> bool {
    PUBLIC_ROUTES.contains(&format!("{method} {path}").as_str())
}

fn is_state_changing(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

async fn code(resp: axum::http::Response<Body>) -> Option<String> {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.ok()?;
    let body: Value = serde_json::from_slice(&bytes).ok()?;
    body["code"].as_str().map(str::to_owned)
}

fn request(method: &str, path: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(concrete(path))
        .header(header::CONTENT_TYPE, "application/json")
}

/// Inserts a verified account with `role` and returns its id.
async fn insert_user(pool: &PgPool, role: &str) -> Uuid {
    let user_role = match role {
        "user" => UserRole::User,
        "admin" => UserRole::Admin,
        "super_admin" => UserRole::SuperAdmin,
        other => panic!("unknown role: {other}"),
    };
    let email = format!("route-guard-{role}-{}@example.com", Uuid::now_v7());
    testing::insert_verified_user(pool, &email, user_role).await
}

/// Session cookie, CSRF cookie and matching header for a session token.
fn with_session(
    builder: axum::http::request::Builder,
    user: Uuid,
    token: &str,
) -> axum::http::request::Builder {
    let csrf = common::mint_csrf_token(&user.to_string());
    builder
        .header(
            header::COOKIE,
            format!("__Host-auth_token={token}; __Host-csrf_token={csrf}"),
        )
        .header("x-csrf-token", csrf)
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_non_public_route_refuses_a_visitor(pool: PgPool) {
    let app = common::create_test_app(pool);
    let stranger = Uuid::now_v7();
    // No session; a token that isn't a token; a well-signed token that has
    // expired. Each with a matching CSRF pair, so state-changing routes get past
    // the header-equals-cookie check and reach session verification.
    let expired = common::mint_session_jwt(stranger, true, "user", -5);
    let credentials: [(&str, Option<&str>); 3] = [
        ("no session", None),
        ("garbage token", Some("not-a-token")),
        ("expired token", Some(expired.as_str())),
    ];
    let mut open = Vec::new();

    for (method, path) in operations() {
        if is_public(&method, &path) {
            continue;
        }
        for (label, token) in credentials {
            let builder = request(&method, &path);
            let builder = match token {
                Some(token) => with_session(builder, stranger, token),
                None => builder,
            };
            let resp = app
                .clone()
                .oneshot(builder.body(Body::from("{}")).unwrap())
                .await
                .unwrap();
            let status = resp.status();
            let refused = status == StatusCode::UNAUTHORIZED
                || (status == StatusCode::FORBIDDEN
                    && is_state_changing(&method)
                    && code(resp).await.as_deref() == Some("csrf_invalid"));
            if !refused {
                open.push(format!("{method} {path} ({label}) -> {status}"));
            }
        }
    }

    assert!(
        open.is_empty(),
        "routes reachable without a valid session (guard them, or add them to \
         PUBLIC_ROUTES with a reason): {open:#?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn every_admin_route_refuses_a_member(pool: PgPool) {
    let member = insert_user(&pool, "user").await;
    let token = common::mint_session_jwt(member, true, "user", 15);
    let app = common::create_test_app(pool);

    let admin_ops: Vec<_> = operations()
        .into_iter()
        .filter(|(_, path)| path == "/api/v1/admin" || path.starts_with("/api/v1/admin/"))
        .collect();
    assert!(
        !admin_ops.is_empty(),
        "the description lists no admin routes"
    );

    let mut open = Vec::new();
    for (method, path) in admin_ops {
        let resp = app
            .clone()
            .oneshot(
                with_session(request(&method, &path), member, &token)
                    .body(Body::from(r#"{"role":"super_admin"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let code = code(resp).await;
        if status != StatusCode::FORBIDDEN || code.as_deref() != Some("access_denied") {
            open.push(format!("{method} {path} -> {status} {code:?}"));
        }
    }

    assert!(
        open.is_empty(),
        "admin routes not refused to a Member with access_denied: {open:#?}"
    );
}

async fn probe() -> &'static str {
    "reached"
}

/// The real admin guard chains around a handler that checks nothing itself.
#[expect(
    clippy::disallowed_methods,
    reason = "test-only probe route with no role extractor, to show the route layer alone \
              enforces the role; never served"
)]
fn guarded_probe(pool: PgPool) -> Router {
    let users: Arc<dyn UserRepository> =
        persistence_postgres::wire(Arc::new(Database::from_pool(pool))).users;
    let probe_routes = || OpenApiRouter::<()>::new().route("/probe", get(probe));
    let (admin, _) = admin_guards(probe_routes()).split_for_parts();
    let (super_admin, _) = super_admin_guards(probe_routes()).split_for_parts();
    Router::new()
        .nest("/admin", admin)
        .nest("/super", super_admin)
        .layer(Extension(common::ensure_test_crypto()))
        .layer(Extension(users))
}

async fn probe_status(
    app: &Router,
    path: &str,
    user: Uuid,
    role: &str,
) -> (StatusCode, Option<String>) {
    let token = common::mint_session_jwt(user, true, role, 15);
    let resp = app
        .clone()
        .oneshot(
            with_session(Request::get(path), user, &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, code(resp).await)
}

#[sqlx::test(migrations = "../../migrations")]
async fn admin_guards_refuse_members_without_any_handler_check(pool: PgPool) {
    let member = insert_user(&pool, "user").await;
    let admin = insert_user(&pool, "admin").await;
    let super_admin = insert_user(&pool, "super_admin").await;
    let app = guarded_probe(pool);
    let denied = (StatusCode::FORBIDDEN, Some("access_denied".to_owned()));
    let reached = (StatusCode::OK, None);

    assert_eq!(
        probe_status(&app, "/admin/probe", member, "user").await,
        denied
    );
    assert_eq!(
        probe_status(&app, "/admin/probe", admin, "admin").await,
        reached
    );
    assert_eq!(
        probe_status(&app, "/admin/probe", super_admin, "super_admin").await,
        reached
    );

    assert_eq!(
        probe_status(&app, "/super/probe", member, "user").await,
        denied
    );
    assert_eq!(
        probe_status(&app, "/super/probe", admin, "admin").await,
        denied
    );
    assert_eq!(
        probe_status(&app, "/super/probe", super_admin, "super_admin").await,
        reached
    );
}

/// A Member just promoted to Admin still holds a token naming the old role. The
/// admin routes must answer 401 (so the client refreshes and retries with the
/// new role), not a 403 from the stale role, which the client wouldn't retry.
#[sqlx::test(migrations = "../../migrations")]
async fn a_just_promoted_member_is_sent_to_refresh_not_refused(pool: PgPool) {
    let promoted = insert_user(&pool, "admin").await;
    let stale_token = common::mint_session_jwt(promoted, true, "user", 15);
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(
            with_session(Request::get("/api/v1/admin/users"), promoted, &stale_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn every_public_route_still_exists() {
    let ops: Vec<String> = operations()
        .into_iter()
        .map(|(method, path)| format!("{method} {path}"))
        .collect();
    let stale: Vec<_> = PUBLIC_ROUTES
        .iter()
        .filter(|route| !ops.iter().any(|op| op == *route))
        .collect();
    assert!(
        stale.is_empty(),
        "PUBLIC_ROUTES names routes the API no longer has: {stale:#?}"
    );
}
