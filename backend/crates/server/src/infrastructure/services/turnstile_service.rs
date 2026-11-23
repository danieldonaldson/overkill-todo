//! Cloudflare Turnstile CAPTCHA verification service.

use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use crate::application::ports::TurnstileService;
use crate::domain::errors::DomainError;

/// Cloudflare Turnstile verification service.
///
/// Verifies tokens by calling the Cloudflare siteverify API.
/// Requires a `TURNSTILE_SECRET_KEY` environment variable.
pub struct CloudflareTurnstileService {
    client: reqwest::Client,
    secret: String,
    /// Siteverify endpoint (default [`DEFAULT_TURNSTILE_VERIFY_URL`]). Overridable
    /// for tests so the client can be pointed at a mock server.
    verify_url: String,
}

const TURNSTILE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const TURNSTILE_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Default Cloudflare Turnstile siteverify endpoint.
pub const DEFAULT_TURNSTILE_VERIFY_URL: &str =
    "https://challenges.cloudflare.com/turnstile/v0/siteverify";

impl CloudflareTurnstileService {
    /// Creates a Cloudflare Turnstile verification client.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built from static timeout configuration.
    #[must_use]
    pub fn new(secret: String) -> Self {
        Self::with_verify_url(secret, DEFAULT_TURNSTILE_VERIFY_URL.to_string())
    }

    /// Creates a client targeting a specific siteverify URL. The production path
    /// uses [`CloudflareTurnstileService::new`]; this exists so tests can redirect
    /// `verify` at a mock server.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built from static timeout configuration.
    #[must_use]
    pub fn with_verify_url(secret: String, verify_url: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(TURNSTILE_REQUEST_TIMEOUT)
                .connect_timeout(TURNSTILE_CONNECT_TIMEOUT)
                .build()
                .expect("Turnstile HTTP client configuration is valid"),
            secret,
            verify_url,
        }
    }
}

#[derive(Deserialize)]
struct TurnstileResponse {
    success: bool,
}

#[async_trait]
impl TurnstileService for CloudflareTurnstileService {
    async fn verify(&self, token: &str) -> Result<bool, DomainError> {
        let response = self
            .client
            .post(&self.verify_url)
            .form(&[("secret", self.secret.as_str()), ("response", token)])
            .send()
            .await
            .map_err(|e| DomainError::ExternalService(format!("Turnstile request failed: {e}")))?
            .json::<TurnstileResponse>()
            .await
            .map_err(|e| {
                DomainError::ExternalService(format!("Turnstile response invalid: {e}"))
            })?;

        Ok(response.success)
    }

    fn is_enabled(&self) -> bool {
        true
    }
}

/// A no-op Turnstile service that always passes.
///
/// Use this when Turnstile is disabled (e.g., in tests or local development).
pub struct NoOpTurnstileService;

#[async_trait]
impl TurnstileService for NoOpTurnstileService {
    async fn verify(&self, _token: &str) -> Result<bool, DomainError> {
        tracing::debug!("Turnstile verification skipped (no-op mode)");
        Ok(true)
    }

    fn is_enabled(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_service_always_passes() {
        let service = NoOpTurnstileService;
        let result = service.verify("any-token").await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn noop_service_passes_with_empty_token() {
        let service = NoOpTurnstileService;
        let result = service.verify("").await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn noop_service_reports_disabled() {
        assert!(!NoOpTurnstileService.is_enabled());
    }

    #[test]
    fn cloudflare_service_reports_enabled() {
        assert!(CloudflareTurnstileService::new("secret".into()).is_enabled());
    }

    #[tokio::test]
    async fn noop_service_passes_with_any_token() {
        let service = NoOpTurnstileService;
        assert!(service.verify("abc123").await.unwrap());
        assert!(service.verify("VALID_TOKEN").await.unwrap());
        assert!(service.verify("0.1234567890").await.unwrap());
        assert!(service.verify("invalid!@#$%").await.unwrap());
    }

    // The siteverify URL is hardcoded to challenges.cloudflare.com, so the async
    // `verify` path can't be redirected at a mock without changing production
    // logic. We pin the wire shape `verify` deserializes instead: the Cloudflare
    // siteverify response carries extra fields we ignore, and only `success`
    // drives the result.

    #[test]
    fn turnstile_response_reads_success_true_ignoring_extra_fields() {
        let parsed: TurnstileResponse = serde_json::from_value(serde_json::json!({
            "success": true,
            "challenge_ts": "2026-06-20T00:00:00Z",
            "hostname": "example.com",
            "error-codes": []
        }))
        .unwrap();
        assert!(parsed.success);
    }

    #[test]
    fn turnstile_response_reads_success_false_with_error_codes() {
        let parsed: TurnstileResponse = serde_json::from_value(serde_json::json!({
            "success": false,
            "error-codes": ["invalid-input-response"]
        }))
        .unwrap();
        assert!(!parsed.success);
    }

    // -------------------------------------------------------------------------
    // Real `verify` path over a wiremock siteverify endpoint.
    // -------------------------------------------------------------------------
    mod over_mock {
        use super::super::*;
        use wiremock::matchers::{method as wm_method, path as wm_path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        fn service_for(server: &MockServer) -> CloudflareTurnstileService {
            CloudflareTurnstileService::with_verify_url(
                "secret".into(),
                format!("{}/siteverify", server.uri()),
            )
        }

        #[tokio::test]
        async fn verify_success_true_returns_ok_true() {
            let server = MockServer::start().await;
            Mock::given(wm_method("POST"))
                .and(wm_path("/siteverify"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "success": true,
                    "hostname": "example.com"
                })))
                .mount(&server)
                .await;

            assert!(service_for(&server).verify("tok").await.unwrap());
        }

        #[tokio::test]
        async fn verify_success_false_returns_ok_false() {
            let server = MockServer::start().await;
            Mock::given(wm_method("POST"))
                .and(wm_path("/siteverify"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "success": false,
                    "error-codes": ["invalid-input-response"]
                })))
                .mount(&server)
                .await;

            assert!(!service_for(&server).verify("tok").await.unwrap());
        }

        #[tokio::test]
        async fn verify_non_json_html_body_maps_to_external_service_error() {
            // A 5xx with an HTML body can't be deserialized into the response
            // struct, so the JSON step fails with an "invalid" external error.
            let server = MockServer::start().await;
            Mock::given(wm_method("POST"))
                .and(wm_path("/siteverify"))
                .respond_with(
                    ResponseTemplate::new(500).set_body_string("<html>Server Error</html>"),
                )
                .mount(&server)
                .await;

            let err = service_for(&server).verify("tok").await.unwrap_err();
            assert!(matches!(err, DomainError::ExternalService(_)));
            assert!(err.to_string().contains("invalid"));
        }

        #[tokio::test]
        async fn verify_transport_error_maps_to_external_service_error() {
            // Point at a port with nothing listening so the request fails to
            // connect, exercising the request-failed error branch.
            let service = CloudflareTurnstileService::with_verify_url(
                "secret".into(),
                "http://127.0.0.1:1/siteverify".into(),
            );
            let err = service.verify("tok").await.unwrap_err();
            assert!(matches!(err, DomainError::ExternalService(_)));
            assert!(err.to_string().contains("failed"));
        }
    }
}
