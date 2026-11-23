//! OAuth start → callback flow through the real router with a fake provider.
//!
//! The fake provider stands in for Google/Apple at the `OAuthProviderClient` port,
//! so these tests drive the *real* (non-bypass) callback path: state JWT
//! verification, the browser-binding nonce cookie, provider matching, and every
//! callback outcome's redirect and cookies.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use application::dto::OAuthUserInfo;
use application::ports::OAuthProviderClient;
use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, Response, StatusCode, header},
};
use domain::entities::OAuthProvider;
use domain::errors::DomainError;
use serde_json::{Value, json};
use server::infrastructure::api::cookies;
use sqlx::PgPool;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Fake provider
// ---------------------------------------------------------------------------

/// A provider whose authorization codes are the `sub` to return, with
/// `email`/`email_verified` fixed per code via a lookup table. Unknown codes
/// fail the exchange like a real provider rejecting a bad code.
struct FakeProvider {
    identities: HashMap<&'static str, OAuthUserInfo>,
}

#[async_trait]
impl OAuthProviderClient for FakeProvider {
    fn authorization_url(&self, state: &str) -> String {
        format!("https://provider.test/authorize?state={state}")
    }

    async fn exchange_code(&self, code: &str) -> Result<OAuthUserInfo, DomainError> {
        self.identities
            .get(code)
            .cloned()
            .ok_or_else(|| DomainError::ExternalService("bad authorization code".into()))
    }
}

fn identity(sub: &str, email: Option<&str>, email_verified: bool) -> OAuthUserInfo {
    OAuthUserInfo {
        sub: sub.to_string(),
        email: email.map(str::to_string),
        email_verified,
        name: None,
        given_name: Some("Olivia".to_string()),
        family_name: Some("Auth".to_string()),
    }
}

/// Codes the fake provider accepts. Each code maps to one identity.
fn identities() -> HashMap<&'static str, OAuthUserInfo> {
    HashMap::from([
        (
            "new-user",
            identity("sub-new", Some("e2e-oauth-new@example.com"), true),
        ),
        ("no-email", identity("sub-no-email", None, true)),
        (
            "existing-email",
            identity("sub-existing", Some("e2e-oauth-existing@example.com"), true),
        ),
        (
            "unverified",
            identity(
                "sub-unverified",
                Some("e2e-oauth-unverified@example.com"),
                false,
            ),
        ),
        (
            "link-me",
            identity("sub-link", Some("e2e-oauth-other@example.com"), true),
        ),
    ])
}

fn app(pool: PgPool) -> Router {
    let make = || -> Arc<dyn OAuthProviderClient> {
        Arc::new(FakeProvider {
            identities: identities(),
        })
    };
    common::create_test_app_with_oauth(
        pool,
        HashMap::from([
            (OAuthProvider::Google, make()),
            (OAuthProvider::Apple, make()),
        ]),
    )
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

async fn body_json(body: Body) -> Value {
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn location(resp: &Response<Body>) -> &str {
    resp.headers()
        .get(header::LOCATION)
        .expect("redirect has a Location")
        .to_str()
        .unwrap()
}

/// All `Set-Cookie` header values, for asserting clears (`Max-Age=0`).
fn set_cookies(resp: &Response<Body>) -> Vec<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

fn assert_handoff_cookies_cleared(resp: &Response<Body>) {
    let cookies = set_cookies(resp);
    for name in [
        cookies::OAUTH_STATE_COOKIE,
        cookies::OAUTH_DEVICE_FINGERPRINT_COOKIE,
    ] {
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with(&format!("{name}=;")) && c.contains("Max-Age=0")),
            "{name} should be cleared, got {cookies:?}"
        );
    }
}

/// A browser's view of a started flow: the `state` the provider will echo back
/// and the binding nonce cookie the server set.
struct Started {
    state: String,
    nonce: String,
}

async fn start(app: &Router, provider: &str, mode: &str, session: Option<&str>) -> Started {
    let mut req = Request::builder().uri(format!("/api/v1/auth/oauth/{provider}?mode={mode}"));
    if let Some(token) = session {
        req = req.header(header::COOKIE, format!("__Host-auth_token={token}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);

    let state = location(&resp)
        .strip_prefix("https://provider.test/authorize?state=")
        .expect("redirects to the provider's authorization URL")
        .to_string();
    let nonce = common::cookie_value(&resp, "__Host-oauth_state").expect("oauth_state cookie set");
    Started { state, nonce }
}

async fn callback(
    app: &Router,
    provider: &str,
    query: &str,
    cookie: Option<&str>,
) -> Response<Body> {
    let mut req = Request::builder().uri(format!("/api/v1/auth/oauth/{provider}/callback?{query}"));
    if let Some(cookie) = cookie {
        req = req.header(header::COOKIE, cookie);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// The callback always answers with a redirect: failures land on the login
/// page with a fixed error code and the handoff cookies cleared.
fn assert_login_error(resp: &Response<Body>, code: &str) {
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(resp), format!("/login?error={code}"));
    assert_handoff_cookies_cleared(resp);
}

/// Run start → callback as one browser and return the callback response.
async fn login_flow(app: &Router, code: &str, device_fingerprint: Option<&str>) -> Response<Body> {
    let s = start(app, "google", "login", None).await;
    let cookie = device_fingerprint.map_or_else(
        || format!("__Host-oauth_state={}", s.nonce),
        |fp| {
            format!(
                "__Host-oauth_state={}; __Host-oauth_device_fingerprint={fp}",
                s.nonce
            )
        },
    );
    callback(
        app,
        "google",
        &format!("code={code}&state={}", s.state),
        Some(&cookie),
    )
    .await
}

async fn me(app: &Router, token: &str) -> Value {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .header(header::COOKIE, format!("__Host-auth_token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-device-fingerprint", "test-device-abc123")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

/// Email signup through to a password; returns the session token. The address
/// must be `e2e-*` so the dev code generator issues the fixed code.
async fn password_signup(app: &Router, email: &str) -> String {
    app.clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    let signup_token = body_json(resp.into_body()).await["signup_token"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": "hunter2hunter2" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    common::cookie_value(&resp, "__Host-auth_token").expect("auth_token cookie")
}

// ---------------------------------------------------------------------------
// Login mode
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn login_creates_user_and_issues_full_session(pool: PgPool) {
    let app = app(pool);

    let resp = login_flow(&app, "new-user", Some("browser-fp")).await;

    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/dashboard");
    assert!(common::cookie_value(&resp, "__Host-csrf_token").is_some());
    assert!(
        common::cookie_value(&resp, "__Host-refresh_token").is_some(),
        "a device fingerprint upgrades the session with a refresh token"
    );
    assert_handoff_cookies_cleared(&resp);

    let token = common::cookie_value(&resp, "__Host-auth_token").expect("auth_token cookie");
    // OT-009: the state round-tripped through Google, and the session it
    // issues is HMAC-SHA-512 under a named key.
    let header = jsonwebtoken::decode_header(&token).unwrap();
    assert_eq!(header.alg, jsonwebtoken::Algorithm::HS512);
    assert!(header.kid.is_some(), "the session token names its key");
    let user = me(&app, &token).await;
    assert_eq!(user["email"], "e2e-oauth-new@example.com");
    assert_eq!(user["email_verified"], true);
    assert_eq!(user["has_password"], false);
    assert_eq!(user["oauth_providers"], json!(["google"]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_without_device_fingerprint_issues_access_only_session(pool: PgPool) {
    let app = app(pool);

    let resp = login_flow(&app, "new-user", None).await;

    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert!(common::cookie_value(&resp, "__Host-auth_token").is_some());
    assert!(common::cookie_value(&resp, "__Host-refresh_token").is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn repeat_login_with_same_sub_returns_same_user(pool: PgPool) {
    let app = app(pool);

    let first = login_flow(&app, "new-user", None).await;
    let first_id = me(
        &app,
        &common::cookie_value(&first, "__Host-auth_token").unwrap(),
    )
    .await["id"]
        .clone();

    let second = login_flow(&app, "new-user", None).await;
    let second_id = me(
        &app,
        &common::cookie_value(&second, "__Host-auth_token").unwrap(),
    )
    .await["id"]
        .clone();

    assert_eq!(first_id, second_id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_auto_links_verified_password_account(pool: PgPool) {
    let app = app(pool);
    let password_token = password_signup(&app, "e2e-oauth-existing@example.com").await;
    let password_user = me(&app, &password_token).await;

    let resp = login_flow(&app, "existing-email", None).await;

    assert_eq!(location(&resp), "/dashboard");
    let user = me(
        &app,
        &common::cookie_value(&resp, "__Host-auth_token").unwrap(),
    )
    .await;
    assert_eq!(user["id"], password_user["id"]);
    assert_eq!(user["oauth_providers"], json!(["google"]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_with_unverified_account_email_redirects_with_conflict(pool: PgPool) {
    let app = app(pool);
    // Registered but never verified: auto-linking would hand the OAuth identity
    // an account whose owner never proved the address.
    app.clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": "e2e-oauth-existing@example.com" }),
        ))
        .await
        .unwrap();

    let resp = login_flow(&app, "existing-email", Some("browser-fp")).await;

    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login?error=oauth_email_conflict");
    assert!(common::cookie_value(&resp, "__Host-auth_token").is_none());
    assert_handoff_cookies_cleared(&resp);
}

#[sqlx::test(migrations = "../../migrations")]
async fn login_without_provider_email_is_rejected(pool: PgPool) {
    let app = app(pool);

    let resp = login_flow(&app, "no-email", None).await;

    assert_login_error(&resp, "oauth_failed");
    assert!(common::cookie_value(&resp, "__Host-auth_token").is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn failed_code_exchange_does_not_log_in(pool: PgPool) {
    let app = app(pool);

    let resp = login_flow(&app, "rejected-by-provider", None).await;

    // An upstream failure still lands the browser back on the login page.
    assert_login_error(&resp, "oauth_failed");
    assert!(common::cookie_value(&resp, "__Host-auth_token").is_none());
}

// ---------------------------------------------------------------------------
// State + browser binding (login CSRF defence)
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn callback_without_state_cookie_is_rejected(pool: PgPool) {
    let app = app(pool);
    // An attacker's valid code+state replayed in a victim's browser: the
    // victim has no `oauth_state` cookie for this flow.
    let s = start(&app, "google", "login", None).await;

    let resp = callback(
        &app,
        "google",
        &format!("code=new-user&state={}", s.state),
        None,
    )
    .await;

    assert_login_error(&resp, "oauth_failed");
    assert!(common::cookie_value(&resp, "__Host-auth_token").is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn callback_with_another_flows_nonce_is_rejected(pool: PgPool) {
    let app = app(pool);
    let attacker = start(&app, "google", "login", None).await;
    let victim = start(&app, "google", "login", None).await;

    let resp = callback(
        &app,
        "google",
        &format!("code=new-user&state={}", attacker.state),
        Some(&format!("__Host-oauth_state={}", victim.nonce)),
    )
    .await;

    assert_login_error(&resp, "oauth_failed");
    assert!(common::cookie_value(&resp, "__Host-auth_token").is_none());
}

/// A cancelled consent arrives as `?error=access_denied&state=…` with no code.
#[sqlx::test(migrations = "../../migrations")]
async fn provider_cancellation_redirects_to_login(pool: PgPool) {
    let app = app(pool);
    let s = start(&app, "google", "login", None).await;

    let resp = callback(
        &app,
        "google",
        &format!("error=access_denied&state={}", s.state),
        Some(&format!("__Host-oauth_state={}", s.nonce)),
    )
    .await;

    assert_login_error(&resp, "oauth_cancelled");
    assert!(common::cookie_value(&resp, "__Host-auth_token").is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn other_provider_error_redirects_to_login(pool: PgPool) {
    let app = app(pool);

    let resp = callback(&app, "google", "error=server_error", None).await;

    assert_login_error(&resp, "oauth_failed");
}

#[sqlx::test(migrations = "../../migrations")]
async fn callback_without_code_redirects_to_login(pool: PgPool) {
    let app = app(pool);
    let s = start(&app, "google", "login", None).await;

    let resp = callback(
        &app,
        "google",
        &format!("state={}", s.state),
        Some(&format!("__Host-oauth_state={}", s.nonce)),
    )
    .await;

    assert_login_error(&resp, "oauth_failed");
}

/// SECURITY: an identity whose provider hasn't verified the email must not
/// create an account. Such a row would be reused by the real owner's email
/// signup, and the provider identity's link would then log into it.
#[sqlx::test(migrations = "../../migrations")]
async fn login_with_unverified_provider_email_creates_no_account(pool: PgPool) {
    let app = app(pool.clone());

    let resp = login_flow(&app, "unverified", Some("browser-fp")).await;

    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login?error=oauth_email_conflict");
    assert!(common::cookie_value(&resp, "__Host-auth_token").is_none());
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn callback_without_state_param_is_rejected(pool: PgPool) {
    let app = app(pool);

    let resp = callback(&app, "google", "code=new-user", None).await;

    assert_login_error(&resp, "oauth_failed");
}

#[sqlx::test(migrations = "../../migrations")]
async fn callback_with_forged_state_is_rejected(pool: PgPool) {
    let app = app(pool);

    let resp = callback(
        &app,
        "google",
        "code=new-user&state=not.a.jwt",
        Some("__Host-oauth_state=whatever"),
    )
    .await;

    assert_login_error(&resp, "oauth_failed");
}

#[sqlx::test(migrations = "../../migrations")]
async fn callback_for_different_provider_than_state_is_rejected(pool: PgPool) {
    let app = app(pool);
    let s = start(&app, "apple", "login", None).await;

    let resp = callback(
        &app,
        "google",
        &format!("code=new-user&state={}", s.state),
        Some(&format!("__Host-oauth_state={}", s.nonce)),
    )
    .await;

    assert_login_error(&resp, "oauth_failed");
}

#[sqlx::test(migrations = "../../migrations")]
async fn callback_with_malformed_user_id_in_state_is_rejected(pool: PgPool) {
    let app = app(pool);
    // A correctly signed state whose `user_id` claim isn't a UUID.
    let crypto = common::ensure_test_crypto();
    let (state, nonce) = server::infrastructure::api::create_oauth_state(
        crypto.tokens(),
        "google",
        "link",
        Some("not-a-uuid"),
    )
    .unwrap();

    let resp = callback(
        &app,
        "google",
        &format!("code=link-me&state={state}"),
        Some(&format!("__Host-oauth_state={nonce}")),
    )
    .await;

    assert_login_error(&resp, "oauth_failed");
}

// ---------------------------------------------------------------------------
// Link mode
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn link_attaches_provider_to_signed_in_user(pool: PgPool) {
    let app = app(pool);
    let token = password_signup(&app, "e2e-oauth-owner@example.com").await;
    let s = start(&app, "apple", "link", Some(&token)).await;

    let resp = callback(
        &app,
        "apple",
        &format!("code=link-me&state={}", s.state),
        Some(&format!("__Host-oauth_state={}", s.nonce)),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/dashboard/settings?oauth=linked");
    assert!(
        common::cookie_value(&resp, "__Host-auth_token").is_none(),
        "linking must not replace the current session"
    );
    assert_handoff_cookies_cleared(&resp);

    let user = me(&app, &token).await;
    assert_eq!(user["email"], "e2e-oauth-owner@example.com");
    assert_eq!(user["oauth_providers"], json!(["apple"]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn relinking_same_identity_to_same_user_is_idempotent(pool: PgPool) {
    let app = app(pool);
    let token = password_signup(&app, "e2e-oauth-owner@example.com").await;

    for _ in 0..2 {
        let s = start(&app, "apple", "link", Some(&token)).await;
        let resp = callback(
            &app,
            "apple",
            &format!("code=link-me&state={}", s.state),
            Some(&format!("__Host-oauth_state={}", s.nonce)),
        )
        .await;
        assert_eq!(location(&resp), "/dashboard/settings?oauth=linked");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn link_identity_owned_by_another_user_redirects_already_linked(pool: PgPool) {
    let app = app(pool);
    // The identity behind "new-user" now belongs to the OAuth-created account.
    login_flow(&app, "new-user", None).await;
    let token = password_signup(&app, "e2e-oauth-owner@example.com").await;
    let s = start(&app, "google", "link", Some(&token)).await;

    let resp = callback(
        &app,
        "google",
        &format!("code=new-user&state={}", s.state),
        Some(&format!("__Host-oauth_state={}", s.nonce)),
    )
    .await;

    assert_eq!(location(&resp), "/login?error=oauth_already_linked");
    assert_handoff_cookies_cleared(&resp);
    assert_eq!(me(&app, &token).await["oauth_providers"], json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn link_with_unverified_provider_email_is_refused(pool: PgPool) {
    let app = app(pool);
    let token = password_signup(&app, "e2e-oauth-owner@example.com").await;
    let s = start(&app, "google", "link", Some(&token)).await;

    let resp = callback(
        &app,
        "google",
        &format!("code=unverified&state={}", s.state),
        Some(&format!("__Host-oauth_state={}", s.nonce)),
    )
    .await;

    assert_eq!(location(&resp), "/login?error=oauth_email_conflict");
    assert_eq!(me(&app, &token).await["oauth_providers"], json!([]));
}

// ---------------------------------------------------------------------------
// Cross-flow: email verification against an OAuth-only account
// ---------------------------------------------------------------------------

/// SECURITY regression: verify-email on an OAuth-only account (verified, no
/// password) used to return a signup token for any code, letting an attacker
/// set a password via complete-signup and take the account over.
#[sqlx::test(migrations = "../../migrations")]
async fn verify_email_cannot_take_over_oauth_only_account(pool: PgPool) {
    let app = app(pool);
    login_flow(&app, "new-user", None).await;
    let email = "e2e-oauth-new@example.com";

    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "000000" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(
        body_json(resp.into_body())
            .await
            .get("signup_token")
            .is_none()
    );

    // And no password can be set on it without a signup token.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": "attackerpass1" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Cookie integrity (P-22)
// ---------------------------------------------------------------------------

/// P-22: every auth cookie the server sets or clears is `__Host-` prefixed,
/// with `Secure`, `Path=/` and no `Domain`, so a browser accepts it only from
/// this host over HTTPS: a sibling host or a plain-HTTP response can't plant
/// one. `models/auth_protocols.spthy` names this test on the rule where the
/// browser stores the app's cookies.
#[sqlx::test(migrations = "../../migrations")]
async fn auth_cookies_are_host_only(pool: PgPool) {
    let app = app(pool);
    let fingerprint = "fp-host-only";
    let mut seen = Vec::new();

    // The device-fingerprint handoff and the start of a Google sign-in.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/oauth/device-fingerprint",
            &json!({ "device_fingerprint": fingerprint }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    seen.extend(set_cookies(&resp));
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/google?mode=login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    seen.extend(set_cookies(&resp));

    // A callback that signs in (sets the session cookies, clears the handoff),
    // and one that fails (clears the handoff).
    let resp = login_flow(&app, "new-user", Some(fingerprint)).await;
    seen.extend(set_cookies(&resp));
    let refresh = common::cookie_value(&resp, cookies::REFRESH_COOKIE).expect("refresh cookie");
    let resp = callback(&app, "google", "code=new-user", None).await;
    seen.extend(set_cookies(&resp));

    // Refresh (a new session) and sign-out (clears it).
    let refresh_req = |token: &str, uri: &str| {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(
                header::COOKIE,
                format!("{}={token}", cookies::REFRESH_COOKIE),
            )
            .header("x-device-fingerprint", fingerprint)
            .body(Body::empty())
            .unwrap()
    };
    let resp = app
        .clone()
        .oneshot(refresh_req(&refresh, "/api/v1/auth/refresh"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    seen.extend(set_cookies(&resp));
    let rotated = common::cookie_value(&resp, cookies::REFRESH_COOKIE).expect("rotated");
    let resp = app
        .clone()
        .oneshot(refresh_req(&rotated, "/api/v1/auth/logout"))
        .await
        .unwrap();
    assert!(resp.status().is_success(), "logout: {}", resp.status());
    seen.extend(set_cookies(&resp));

    // Every cookie was both set and cleared along the way.
    for name in [
        cookies::AUTH_COOKIE,
        cookies::REFRESH_COOKIE,
        cookies::CSRF_COOKIE,
        cookies::OAUTH_STATE_COOKIE,
        cookies::OAUTH_DEVICE_FINGERPRINT_COOKIE,
    ] {
        let set = seen
            .iter()
            .any(|c| c.starts_with(&format!("{name}=")) && !c.starts_with(&format!("{name}=;")));
        let cleared = seen
            .iter()
            .any(|c| c.starts_with(&format!("{name}=;")) && c.contains("Max-Age=0"));
        assert!(
            set && cleared,
            "{name}: set {set}, cleared {cleared}, in {seen:?}"
        );
    }
    for c in &seen {
        let attrs: Vec<String> = c
            .split(';')
            .skip(1)
            .map(|a| a.trim().to_ascii_lowercase())
            .collect();
        assert!(c.starts_with("__Host-"), "not host-prefixed: {c}");
        assert!(attrs.iter().any(|a| a == "secure"), "not Secure: {c}");
        assert!(attrs.iter().any(|a| a == "path=/"), "Path isn't /: {c}");
        assert!(
            !attrs.iter().any(|a| a.starts_with("domain")),
            "has a Domain: {c}"
        );
    }
}
