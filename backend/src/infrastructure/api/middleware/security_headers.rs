//! Middleware that attaches baseline security headers to every response.

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

use crate::infrastructure::config::is_dev_mode;

// `Strict-Transport-Security` is only emitted outside dev mode, where TLS is
// present. 2 years, includeSubDomains — a conventional production value.
const HSTS_VALUE: &str = "max-age=63072000; includeSubDomains";

/// Adds security headers to the outgoing response.
///
/// - `X-Content-Type-Options: nosniff` — disable MIME sniffing
/// - `X-Frame-Options: DENY` — block framing (clickjacking)
/// - `Referrer-Policy: no-referrer` — never leak URLs as referrers
/// - `Cross-Origin-Opener-Policy: same-origin` — isolate the browsing context
/// - `Strict-Transport-Security` — force HTTPS (production only)
pub async fn security_headers_middleware(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let headers = res.headers_mut();

    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );

    if !is_dev_mode() {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(HSTS_VALUE),
        );
    }

    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, routing::get};
    use tower::ServiceExt;

    #[tokio::test]
    async fn sets_baseline_security_headers() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(security_headers_middleware));

        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let h = res.headers();
        assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
        assert_eq!(h.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
        assert_eq!(h.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
        assert_eq!(h.get("cross-origin-opener-policy").unwrap(), "same-origin");
        // DEV_MODE is unset in tests → treated as production → HSTS present.
        assert_eq!(
            h.get(header::STRICT_TRANSPORT_SECURITY).unwrap(),
            HSTS_VALUE
        );
    }
}
