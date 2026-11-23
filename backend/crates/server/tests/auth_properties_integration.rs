//! Threat-model properties that needed an end-to-end test (OT-010 slice 2).
//! Each test names the property it checks in `docs/security/threat-model.md`.

mod common;

use std::fmt::Write as _;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use domain::entities::OAuthProvider;
use persistence_postgres::testing;
use serde_json::{Value, json};
use server::infrastructure::api::cookies;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const FINGERPRINT: &str = "test-device-auth-properties";
const PASSWORD: &str = "hunter2hunter2";

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-device-fingerprint", FINGERPRINT)
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (axum::http::Response<Body>, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (axum::http::Response::from_parts(parts, Body::empty()), json)
}

struct Session {
    auth: String,
    refresh: String,
    csrf: String,
}

/// Signs up `email` (a `test-` address, so the dev code is `123456`) with
/// [`PASSWORD`] and signs in, returning the session a
/// browser would hold.
async fn signed_in(app: &Router, email: &str) -> Session {
    send(
        app,
        post_json("/api/v1/auth/register", &json!({ "email": email })),
    )
    .await;
    let (_, body) = send(
        app,
        post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ),
    )
    .await;
    let signup_token = body["signup_token"]
        .as_str()
        .unwrap_or_else(|| panic!("verify-email: {body}"));
    send(
        app,
        post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": PASSWORD }),
        ),
    )
    .await;
    login(app, email, PASSWORD)
        .await
        .unwrap_or_else(|status| panic!("login for {email} failed: {status}"))
}

async fn login(app: &Router, email: &str, password: &str) -> Result<Session, StatusCode> {
    let (resp, body) = send(
        app,
        post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": password }),
        ),
    )
    .await;
    if resp.status() != StatusCode::OK {
        return Err(resp.status());
    }
    Ok(Session {
        auth: common::cookie_value(&resp, "__Host-auth_token").expect("auth cookie"),
        refresh: common::cookie_value(&resp, "__Host-refresh_token").expect("refresh cookie"),
        csrf: body["csrf_token"].as_str().unwrap().to_string(),
    })
}

/// Presents `s`'s refresh token; returns the status and the rotated refresh
/// token, if a new session was issued.
async fn try_refresh(app: &Router, s: &Session) -> (StatusCode, Option<String>) {
    try_refresh_from(app, s, FINGERPRINT).await
}

/// Like [`try_refresh`], from a device with `fingerprint`.
async fn try_refresh_from(
    app: &Router,
    s: &Session,
    fingerprint: &str,
) -> (StatusCode, Option<String>) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/refresh")
        .header(
            header::COOKIE,
            format!(
                "__Host-refresh_token={}; __Host-csrf_token={}",
                s.refresh, s.csrf
            ),
        )
        .header("x-csrf-token", &s.csrf)
        .header("x-device-fingerprint", fingerprint)
        .body(Body::empty())
        .unwrap();
    let (resp, _) = send(app, req).await;
    let rotated = common::cookie_value(&resp, "__Host-refresh_token").filter(|t| !t.is_empty());
    (resp.status(), rotated)
}

/// A request carrying `s`'s session cookie, CSRF cookie and CSRF header.
fn authed(method: &str, uri: &str, s: &Session, body: Option<&Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!("__Host-auth_token={}; __Host-csrf_token={}", s.auth, s.csrf),
        )
        .header("x-csrf-token", &s.csrf)
        .header("x-device-fingerprint", FINGERPRINT);
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

/// Asks for a password-reset code (the dev code `123456` for `test-` addresses).
async fn request_reset(app: &Router, email: &str) {
    let (resp, _) = send(
        app,
        post_json("/api/v1/auth/forgot-password", &json!({ "email": email })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Exchanges the reset code for a reset token and sets `new_password`;
/// returns the reset token.
async fn complete_reset(app: &Router, email: &str, new_password: &str) -> String {
    let (_, body) = send(
        app,
        post_json(
            "/api/v1/auth/verify-reset-code",
            &json!({ "email": email, "code": "123456" }),
        ),
    )
    .await;
    let reset_token = body["reset_token"]
        .as_str()
        .unwrap_or_else(|| panic!("verify-reset-code: {body}"))
        .to_owned();
    let (resp, body) = send(
        app,
        post_json(
            "/api/v1/auth/reset-password",
            &json!({ "reset_token": reset_token, "new_password": new_password }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "reset-password: {body}");
    reset_token
}

// ── P-04 ─────────────────────────────────────────────────────────────────────

/// P-04: after a password reset, the refresh token issued before it is refused,
/// and so is the old session on `/auth/me` and Google unlink (that it's refused on the
/// account-sensitive routes is `verification_integration::
/// password_reset_flow_succeeds`).
#[sqlx::test(migrations = "../../migrations")]
async fn a_password_reset_ends_earlier_sessions(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "test-p04-reset@example.com";
    let before = signed_in(&app, email).await;

    request_reset(&app, email).await;
    complete_reset(&app, email, "brandnewpass9").await;

    let (status, rotated) = try_refresh(&app, &before).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "pre-reset refresh token");
    assert!(
        rotated.is_none(),
        "a pre-reset refresh token must not yield a session"
    );

    let (resp, _) = send(&app, authed("GET", "/api/v1/auth/me", &before, None)).await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "pre-reset session on /auth/me"
    );

    let (resp, _) = send(
        &app,
        authed(
            "DELETE",
            "/api/v1/auth/oauth/google/unlink",
            &before,
            Some(&json!({})),
        ),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "pre-reset session on Google unlink"
    );
}

// ── P-17 ─────────────────────────────────────────────────────────────────────

/// P-17: signing out revokes the refresh token server-side, not just the cookie.
#[sqlx::test(migrations = "../../migrations")]
async fn a_refresh_token_is_refused_after_logout(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signed_in(&app, "test-p17-logout@example.com").await;

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/logout")
        .header(
            header::COOKIE,
            format!(
                "__Host-auth_token={}; __Host-refresh_token={}; __Host-csrf_token={}",
                session.auth, session.refresh, session.csrf
            ),
        )
        .body(Body::empty())
        .unwrap();
    let (resp, _) = send(&app, req).await;
    assert!(resp.status().is_success(), "logout: {}", resp.status());

    let (status, rotated) = try_refresh(&app, &session).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        rotated.is_none(),
        "a signed-out refresh token must not yield a session"
    );
}

// ── P-15, P-16 (refresh checks that models/auth_protocols.spthy relies on) ───

/// P-15: presenting a refresh token after it was rotated revokes every session
/// of that Member: the rotated successor and an unrelated sign-in alike.
#[sqlx::test(migrations = "../../migrations")]
async fn reusing_a_rotated_refresh_token_ends_every_session(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "test-p15-reuse@example.com";
    let first = signed_in(&app, email).await;
    let other = login(&app, email, PASSWORD).await.expect("second sign-in");

    // The owner refreshes, rotating the first token.
    let (status, rotated) = try_refresh(&app, &first).await;
    assert_eq!(status, StatusCode::OK);
    let successor = Session {
        auth: first.auth.clone(),
        refresh: rotated.expect("a rotated refresh token"),
        csrf: first.csrf.clone(),
    };

    // The rotated token is presented again: refused as reuse.
    let (status, issued) = try_refresh(&app, &first).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(issued.is_none(), "a rotated token must not yield a session");

    // Every session of the Member has ended.
    for (name, session) in [("the successor", &successor), ("the other sign-in", &other)] {
        let (status, issued) = try_refresh(&app, session).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{name} still refreshes");
        assert!(issued.is_none(), "{name} still yields a session");
    }
}

/// P-16: a refresh token presented from another device is refused, and still
/// works from the device it was issued to.
#[sqlx::test(migrations = "../../migrations")]
async fn a_refresh_token_is_refused_from_another_device(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signed_in(&app, "test-p16-fingerprint@example.com").await;

    let (status, issued) = try_refresh_from(&app, &session, "another-device").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(issued.is_none(), "another device must not get a session");

    let (status, issued) = try_refresh(&app, &session).await;
    assert_eq!(status, StatusCode::OK, "the issuing device is refused too");
    assert!(issued.is_some());
}

// ── P-09 ─────────────────────────────────────────────────────────────────────

/// Everything stored about one account, as text, for before/after comparison.
async fn snapshot(pool: &PgPool, id: Uuid) -> String {
    testing::account_snapshot_text(pool, id).await
}

async fn user_id(pool: &PgPool, email: &str) -> Uuid {
    testing::user_id_by_email(pool, email).await
}

/// P-09: as Member A, call every account route (the `/users` routes and Google
/// unlink) with Member B's id and email in the query and in extra body fields.
/// Each call acts on A or is refused; B's stored data (user row, linked
/// identity, sessions) is unchanged and never appears in A's responses.
///
/// B is made password-less with a linked Google identity, so that a route
/// wrongly acting on B would visibly succeed: set-password would give B a
/// password, unlink would remove B's identity.
#[sqlx::test(migrations = "../../migrations")]
async fn a_member_reaches_only_their_own_account(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let a_email = "test-p09-alice@example.com";
    let b_email = "test-p09-bob-distinctive@example.com";
    let a = signed_in(&app, a_email).await;
    let b = signed_in(&app, b_email).await;
    let a_id = user_id(&pool, a_email).await;
    let b_id = user_id(&pool, b_email).await;

    testing::set_passwordless_user_name(&pool, b_id, "Bobsled", "Zyxwvut").await;
    testing::insert_oauth_account(
        &pool,
        b_id,
        OAuthProvider::Google,
        "p09-bob-google-sub",
        "bob-google@example.com",
    )
    .await;
    let b_before = snapshot(&pool, b_id).await;
    let b_markers = [
        b_id.to_string(),
        b_email.to_owned(),
        "Bobsled".to_owned(),
        "Zyxwvut".to_owned(),
        "p09-bob-google-sub".to_owned(),
        "bob-google@example.com".to_owned(),
    ];
    let q = format!("user_id={b_id}&id={b_id}&email={b_email}");
    let other = json!({ "user_id": b_id, "id": b_id, "email": b_email });
    let with_other = |fields: Value| {
        let mut body = other.clone();
        body.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        body
    };
    let a_scheduled = || async {
        testing::user_snapshot(&pool, a_id)
            .await
            .deletion_scheduled_at
    };

    // Export: A's data only.
    let req = authed("GET", &format!("/api/v1/users/me/export?{q}"), &a, None);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let export = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(export.contains(a_email), "export is A's: {export}");
    for marker in &b_markers {
        assert!(
            !export.contains(marker.as_str()),
            "export leaks B's {marker}: {export}"
        );
    }

    // Set password: A already has one, so this is refused. Acting on B
    // (password-less) would succeed and change B's row.
    let (resp, _) = send(
        &app,
        authed(
            "POST",
            &format!("/api/v1/users/password/set?{q}"),
            &a,
            Some(&with_other(json!({ "new_password": "setbyalice1" }))),
        ),
    )
    .await;
    assert!(
        resp.status().is_client_error(),
        "set-password: {}",
        resp.status()
    );

    // Unlink Google: A has no link, so this is refused. Acting on B would
    // remove B's linked identity.
    let (resp, _) = send(
        &app,
        authed(
            "DELETE",
            &format!("/api/v1/auth/oauth/google/unlink?{q}"),
            &a,
            Some(&other),
        ),
    )
    .await;
    assert!(resp.status().is_client_error(), "unlink: {}", resp.status());

    // Schedule deletion: A's account.
    let (resp, _) = send(
        &app,
        authed(
            "DELETE",
            &format!("/api/v1/users/me?{q}"),
            &a,
            Some(&with_other(
                json!({ "password": PASSWORD, "confirmation": "DELETE" }),
            )),
        ),
    )
    .await;
    assert!(
        resp.status().is_success(),
        "request deletion: {}",
        resp.status()
    );
    assert!(a_scheduled().await.is_some(), "A's deletion is scheduled");

    // Cancel deletion: A's again.
    let (resp, _) = send(
        &app,
        authed(
            "POST",
            &format!("/api/v1/users/me/cancel-deletion?{q}"),
            &a,
            Some(&other),
        ),
    )
    .await;
    assert!(
        resp.status().is_success(),
        "cancel deletion: {}",
        resp.status()
    );
    assert!(a_scheduled().await.is_none(), "A's deletion is cancelled");

    // Change password last: it revokes A's other sessions.
    let (resp, _) = send(
        &app,
        authed(
            "POST",
            &format!("/api/v1/users/password/change?{q}"),
            &a,
            Some(&with_other(
                json!({ "current_password": PASSWORD, "new_password": "alicesnewpass1" }),
            )),
        ),
    )
    .await;
    assert!(
        resp.status().is_success(),
        "change password: {}",
        resp.status()
    );
    assert!(
        login(&app, a_email, "alicesnewpass1").await.is_ok(),
        "A's password changed"
    );

    // B's user row, linked identity and sessions are exactly as before.
    assert_eq!(snapshot(&pool, b_id).await, b_before, "B's account changed");
    let (status, _) = try_refresh(&app, &b).await;
    assert_eq!(status, StatusCode::OK, "B's original session still works");
}

// ── P-21 ─────────────────────────────────────────────────────────────────────

const OTHER_ORIGIN: &str = "https://attacker.example";

/// CSRF-exempt routes that read a JSON body, with a body that would succeed if
/// it were accepted.
fn exempt_body_routes() -> Vec<(&'static str, Value)> {
    let email = "test-p21-victim@example.com";
    vec![
        (
            "/api/v1/auth/login",
            json!({ "email": email, "password": PASSWORD }),
        ),
        ("/api/v1/auth/register", json!({ "email": email })),
        (
            "/api/v1/auth/verify-email",
            json!({ "email": email, "code": "123456" }),
        ),
        ("/api/v1/auth/verify-magic", json!({ "token": "x" })),
        (
            "/api/v1/auth/resend-verification",
            json!({ "email": email }),
        ),
        ("/api/v1/auth/forgot-password", json!({ "email": email })),
        (
            "/api/v1/auth/verify-reset-code",
            json!({ "email": email, "code": "123456" }),
        ),
        (
            "/api/v1/auth/reset-password",
            json!({ "reset_token": "x", "new_password": "longenough1" }),
        ),
        (
            "/api/v1/auth/complete-signup",
            json!({ "signup_token": "x", "email": email, "password": PASSWORD }),
        ),
        (
            "/api/v1/auth/oauth/device-fingerprint",
            json!({ "device_fingerprint": "attacker-chosen" }),
        ),
    ]
}

/// The bodies a cross-site HTML form can send without a CORS preflight.
fn form_bodies(fields: &Value) -> Vec<(&'static str, String)> {
    let pairs: Vec<String> = fields
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
        .collect();
    let mut multipart = String::new();
    for (k, v) in fields.as_object().unwrap() {
        let _ = write!(
            multipart,
            "--b\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{}\r\n",
            v.as_str().unwrap_or_default()
        );
    }
    multipart.push_str("--b--\r\n");
    vec![
        // A `text/plain` form can carry a JSON-shaped body verbatim.
        ("text/plain", fields.to_string()),
        ("application/x-www-form-urlencoded", pairs.join("&")),
        ("multipart/form-data; boundary=b", multipart),
    ]
}

fn sets_credential_cookie(resp: &axum::http::Response<Body>) -> bool {
    [
        cookies::AUTH_COOKIE,
        cookies::REFRESH_COOKIE,
        cookies::CSRF_COOKIE,
        cookies::OAUTH_DEVICE_FINGERPRINT_COOKIE,
    ]
    .iter()
    .any(|name| common::cookie_value(resp, name).is_some_and(|v| !v.is_empty()))
}

/// P-21: the CSRF-exempt routes can't be driven by a cross-site form. Routes
/// with a body accept only `application/json`; refresh needs a custom header;
/// and no response grants another origin CORS access.
#[sqlx::test(migrations = "../../migrations")]
async fn csrf_exempt_routes_cannot_be_driven_cross_site(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    // A real account, so a login or code that slipped through would succeed.
    let victim = signed_in(&app, "test-p21-victim@example.com").await;
    let mut accepted = Vec::new();

    for (path, fields) in exempt_body_routes() {
        for (content_type, body) in form_bodies(&fields) {
            let req = Request::builder()
                .method("POST")
                .uri(path)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::ORIGIN, OTHER_ORIGIN)
                .body(Body::from(body))
                .unwrap();
            let (resp, json) = send(&app, req).await;
            let status = resp.status();
            if !status.is_client_error() || sets_credential_cookie(&resp) {
                accepted.push(format!("{path} as {content_type} -> {status}"));
            } else if status != StatusCode::BAD_REQUEST || json["code"] != "validation_error" {
                // Refused, but not with the standard JSON 400 every malformed
                // input gets (threat model P-32).
                accepted.push(format!(
                    "{path} as {content_type} -> {status} without the JSON 400: {json}"
                ));
            }
        }
    }

    // A cross-site form can't send `x-device-fingerprint`; the browser would
    // send the refresh cookie only if SameSite allowed it, so assume it does.
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/refresh")
        .header(
            header::COOKIE,
            format!("__Host-refresh_token={}", victim.refresh),
        )
        .header(header::ORIGIN, OTHER_ORIGIN)
        .body(Body::empty())
        .unwrap();
    let (resp, _) = send(&app, req).await;
    if !resp.status().is_client_error() || sets_credential_cookie(&resp) {
        accepted.push(format!(
            "refresh without fingerprint header -> {}",
            resp.status()
        ));
    }

    // No account was created by a form-encoded sign-up for a new address.
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/register")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(
            json!({ "email": "test-p21-new@example.com" }).to_string(),
        ))
        .unwrap();
    send(&app, req).await;
    let created = testing::count_users_by_email(&pool, "test-p21-new@example.com").await;
    if created != 0 {
        accepted.push("text/plain sign-up created an account".to_owned());
    }

    assert!(
        accepted.is_empty(),
        "cross-site requests accepted: {accepted:#?}"
    );
}

/// P-21: CORS never grants another origin, for preflights or actual requests.
#[sqlx::test(migrations = "../../migrations")]
async fn cors_grants_no_other_origin(pool: PgPool) {
    let app = common::create_test_app(pool);
    let mut granted = Vec::new();

    for (path, _) in exempt_body_routes() {
        let preflight = Request::builder()
            .method("OPTIONS")
            .uri(path)
            .header(header::ORIGIN, OTHER_ORIGIN)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
            .body(Body::empty())
            .unwrap();
        let (resp, _) = send(&app, preflight).await;
        if let Some(allowed) = resp.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            && (allowed == OTHER_ORIGIN || allowed == "*")
        {
            granted.push(format!("preflight {path} -> {allowed:?}"));
        }

        let actual = post_json(path, &json!({}));
        let (mut parts, body) = actual.into_parts();
        parts
            .headers
            .insert(header::ORIGIN, OTHER_ORIGIN.parse().unwrap());
        let (resp, _) = send(&app, Request::from_parts(parts, body)).await;
        if let Some(allowed) = resp.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            && (allowed == OTHER_ORIGIN || allowed == "*")
        {
            granted.push(format!("POST {path} -> {allowed:?}"));
        }
    }

    assert!(
        granted.is_empty(),
        "CORS granted another origin: {granted:#?}"
    );
}

// ── P-27 ─────────────────────────────────────────────────────────────────────

/// Every row of every table in the public schema, as text.
async fn dump_database(pool: &PgPool) -> String {
    testing::dump_database(pool).await
}

/// Wraps the console email service and keeps every magic link it is asked to
/// send, so the test sees the raw magic-link token a real inbox would get.
#[derive(Default)]
struct CapturingEmail {
    magic_links: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl application::ports::outgoing::EmailService for CapturingEmail {
    async fn send_verification_email(
        &self,
        _to: &str,
        _code: &str,
        _first_name: &str,
        magic_link: Option<String>,
    ) -> Result<(), domain::errors::DomainError> {
        if let Some(link) = magic_link {
            self.magic_links.lock().unwrap().push(link);
        }
        Ok(())
    }

    async fn send_password_reset_email(
        &self,
        _to: &str,
        _code: &str,
        _first_name: &str,
    ) -> Result<(), domain::errors::DomainError> {
        Ok(())
    }

    async fn notify_account_exists(
        &self,
        _to: &str,
        _first_name: &str,
    ) -> Result<(), domain::errors::DomainError> {
        Ok(())
    }
}

/// P-27: a copy of the database, taken at any point in sign-up, verification,
/// sign-in, refresh and a password reset, holds none of the passwords, codes
/// or tokens those flows handled: only hashes of them. The database is dumped
/// while each code is still live (after it is sent, before it is used), since
/// using a code deletes its row.
///
/// Google linking is not part of this flow: it stores only the provider's
/// user id and email, and no provider token or secret (threat model P-27).
#[sqlx::test(migrations = "../../migrations")]
async fn a_database_copy_holds_no_usable_credentials(pool: PgPool) {
    let email_service = std::sync::Arc::new(CapturingEmail::default());
    let app = common::create_test_app_with_email_service(pool.clone(), email_service.clone());
    let email = "test-p27-secrets@example.com";
    let code = "123456"; // the dev code for `test-` addresses
    let mut secrets: Vec<(&str, String)> =
        vec![("password", PASSWORD.to_owned()), ("code", code.to_owned())];
    let mut dumps = String::new();

    // Sign-up: the verification code and magic-link token are live.
    send(
        &app,
        post_json("/api/v1/auth/register", &json!({ "email": email })),
    )
    .await;
    dumps.push_str(&dump_database(&pool).await);
    let magic_link = email_service
        .magic_links
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("a magic link was sent");
    let magic_token = magic_link
        .split_once("token=")
        .map(|(_, t)| t.split(['&', '#']).next().unwrap_or_default().to_owned())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| panic!("no token in {magic_link}"));
    secrets.push(("magic-link token", magic_token));

    let (_, body) = send(
        &app,
        post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": code }),
        ),
    )
    .await;
    let signup_token = body["signup_token"]
        .as_str()
        .expect("signup token")
        .to_owned();
    secrets.push(("signup token", signup_token.clone()));
    send(
        &app,
        post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": PASSWORD }),
        ),
    )
    .await;

    // Sign-in and refresh: the session and both refresh tokens.
    let session = login(&app, email, PASSWORD).await.expect("login");
    let (status, rotated) = try_refresh(&app, &session).await;
    assert_eq!(status, StatusCode::OK, "refresh");
    secrets.push(("access token", session.auth.clone()));
    secrets.push(("refresh token", session.refresh.clone()));
    secrets.push(("csrf token", session.csrf.clone()));
    secrets.push((
        "rotated refresh token",
        rotated.expect("rotated refresh token"),
    ));

    // Password reset: the reset code is live, then the reset token is used.
    request_reset(&app, email).await;
    dumps.push_str(&dump_database(&pool).await);
    let new_password = "p27-new-secret-pass";
    secrets.push((
        "reset token",
        complete_reset(&app, email, new_password).await,
    ));
    secrets.push(("new password", new_password.to_owned()));
    dumps.push_str(&dump_database(&pool).await);

    assert!(dumps.contains(email), "the dumps cover this account");
    assert!(
        dumps.contains("email_verification_codes") && dumps.contains("password_reset_codes"),
        "the dumps caught both codes while they were live"
    );
    let leaked: Vec<_> = secrets
        .iter()
        .filter(|(_, value)| dumps.contains(value.as_str()))
        .map(|(name, _)| *name)
        .collect();
    assert!(leaked.is_empty(), "stored in plain text: {leaked:?}");
}
