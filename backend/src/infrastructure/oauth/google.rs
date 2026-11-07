//! Google OAuth client implementation.

use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{JwkSet, KeyAlgorithm},
};
use reqwest::Client;
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use async_trait::async_trait;

use crate::application::dto::OAuthUserInfo;
use crate::application::ports::OAuthProviderClient;
use crate::domain::errors::DomainError;

#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    id_token: String,
    #[allow(dead_code)]
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct GoogleIdTokenClaims {
    sub: String,
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    given_name: Option<String>,
    family_name: Option<String>,
    #[allow(dead_code)]
    iss: String,
    #[allow(dead_code)]
    aud: String,
    #[allow(dead_code)]
    exp: u64,
}

/// A fetched JWKS together with the instant it was retrieved, used to age out
/// the in-memory cache.
struct CachedJwks {
    keys: JwkSet,
    fetched_at: Instant,
}

/// Google OAuth 2.0 client.
pub struct GoogleOAuthClient {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    http_client: Client,
    /// Token-exchange endpoint (default [`GOOGLE_TOKEN_URL`]). Overridable for
    /// tests so `exchange_code` can be pointed at a mock server.
    token_url: String,
    /// JWKS endpoint (default [`GOOGLE_JWKS_URL`]). Overridable for tests so the
    /// JWKS fetch can be pointed at a mock server.
    jwks_url: String,
    /// Cached Google signing keys. Google rotates these only occasionally and
    /// serves them with multi-hour `Cache-Control`, so refetching on every
    /// login is wasteful; we cache for [`GOOGLE_JWKS_TTL`] and refetch on a
    /// cache miss (stale entry or an unknown `kid` after a key rotation).
    jwks_cache: RwLock<Option<CachedJwks>>,
}

const GOOGLE_OAUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const GOOGLE_OAUTH_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const GOOGLE_JWKS_TTL: Duration = Duration::from_hours(1);
const GOOGLE_ID_TOKEN_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];

impl GoogleOAuthClient {
    /// Creates a new Google OAuth client targeting the real Google endpoints.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built from static timeout configuration.
    #[must_use]
    pub fn new(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::with_endpoints(
            client_id,
            client_secret,
            redirect_uri,
            GOOGLE_TOKEN_URL.to_string(),
            GOOGLE_JWKS_URL.to_string(),
        )
    }

    /// Creates a client targeting specific token/JWKS endpoints. The production
    /// path uses [`GoogleOAuthClient::new`]; this exists so tests can redirect the
    /// network calls at a mock server.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built from static timeout configuration.
    #[must_use]
    pub fn with_endpoints(
        client_id: String,
        client_secret: String,
        redirect_uri: String,
        token_url: String,
        jwks_url: String,
    ) -> Self {
        Self {
            client_id,
            client_secret,
            redirect_uri,
            http_client: Client::builder()
                .timeout(GOOGLE_OAUTH_REQUEST_TIMEOUT)
                .connect_timeout(GOOGLE_OAUTH_CONNECT_TIMEOUT)
                .build()
                .expect("Google OAuth HTTP client configuration is valid"),
            token_url,
            jwks_url,
            jwks_cache: RwLock::new(None),
        }
    }

    /// Returns a JWKS known to contain `kid`, serving a fresh cached copy when
    /// possible and refetching from Google only on a miss (stale entry or a
    /// `kid` absent from the cache, e.g. after a key rotation).
    async fn jwks_for_kid(&self, kid: &str) -> Result<JwkSet, DomainError> {
        if let Some(cached) = self.cached_jwks().await
            && cached.find(kid).is_some()
        {
            return Ok(cached);
        }
        self.fetch_and_cache_jwks().await
    }

    /// Returns the cached JWKS if present and within [`GOOGLE_JWKS_TTL`].
    async fn cached_jwks(&self) -> Option<JwkSet> {
        let guard = self.jwks_cache.read().await;
        guard
            .as_ref()
            .filter(|c| c.fetched_at.elapsed() < GOOGLE_JWKS_TTL)
            .map(|c| c.keys.clone())
    }

    async fn fetch_and_cache_jwks(&self) -> Result<JwkSet, DomainError> {
        let keys: JwkSet = self
            .http_client
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| DomainError::ExternalService(format!("Google JWKS fetch failed: {e}")))?
            .error_for_status()
            .map_err(|e| DomainError::ExternalService(format!("Google JWKS fetch failed: {e}")))?
            .json()
            .await
            .map_err(|e| {
                DomainError::ExternalService(format!("Failed to parse Google JWKS: {e}"))
            })?;

        *self.jwks_cache.write().await = Some(CachedJwks {
            keys: keys.clone(),
            fetched_at: Instant::now(),
        });
        Ok(keys)
    }

    fn verify_id_token(
        &self,
        id_token: &str,
        jwks: &JwkSet,
    ) -> Result<GoogleIdTokenClaims, DomainError> {
        let header = decode_header(id_token).map_err(|e| {
            DomainError::ExternalService(format!("Invalid Google ID token header: {e}"))
        })?;
        let kid = header
            .kid
            .ok_or_else(|| DomainError::ExternalService("Google ID token missing key id".into()))?;
        let jwk = jwks.find(&kid).ok_or_else(|| {
            DomainError::ExternalService("Google ID token key id not found".into())
        })?;

        if jwk.common.key_algorithm != Some(KeyAlgorithm::RS256) {
            return Err(DomainError::ExternalService(
                "Google ID token key is not RS256".into(),
            ));
        }

        let decoding_key = DecodingKey::from_jwk(jwk)
            .map_err(|e| DomainError::ExternalService(format!("Invalid Google JWKS key: {e}")))?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[self.client_id.as_str()]);
        validation.set_issuer(&GOOGLE_ID_TOKEN_ISSUERS);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

        decode::<GoogleIdTokenClaims>(id_token, &decoding_key, &validation)
            .map(|token| token.claims)
            .map_err(|e| DomainError::ExternalService(format!("Invalid Google ID token: {e}")))
    }
}

#[async_trait]
impl OAuthProviderClient for GoogleOAuthClient {
    /// Build the authorization URL to redirect the user to Google.
    fn authorization_url(&self, state: &str) -> String {
        let params = [
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", "openid email profile"),
            ("state", state),
            ("access_type", "offline"),
            ("prompt", "select_account"),
        ];

        let query = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        format!("https://accounts.google.com/o/oauth2/v2/auth?{query}")
    }

    /// Exchange authorization code for tokens and extract user info.
    #[tracing::instrument(skip(self, code))]
    async fn exchange_code(&self, code: &str) -> Result<OAuthUserInfo, DomainError> {
        // Exchange code for tokens
        let response = self
            .http_client
            .post(&self.token_url)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("grant_type", "authorization_code"),
                ("code", code),
            ])
            .send()
            .await
            .map_err(|e| {
                DomainError::ExternalService(format!("Google token exchange failed: {e}"))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::error!(status = %status, body = %body, "Google token exchange failed");
            return Err(DomainError::ExternalService(format!(
                "Google token exchange failed: {status}"
            )));
        }

        let token_response: GoogleTokenResponse = response.json().await.map_err(|e| {
            DomainError::ExternalService(format!("Failed to parse Google token response: {e}"))
        })?;

        let header = decode_header(&token_response.id_token).map_err(|e| {
            DomainError::ExternalService(format!("Invalid Google ID token header: {e}"))
        })?;
        let kid = header
            .kid
            .ok_or_else(|| DomainError::ExternalService("Google ID token missing key id".into()))?;
        let jwks = self.jwks_for_kid(&kid).await?;
        let claims = self.verify_id_token(&token_response.id_token, &jwks)?;

        Ok(OAuthUserInfo {
            sub: claims.sub,
            email: claims.email,
            email_verified: claims.email_verified.unwrap_or(false),
            name: claims.name,
            given_name: claims.given_name,
            family_name: claims.family_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::crypto::Crypto;
    use jsonwebtoken::{EncodingKey, Header, encode, get_current_timestamp, jwk::Jwk};
    use serde::Serialize;

    #[derive(Serialize)]
    struct TestGoogleClaims<'a> {
        sub: &'a str,
        email: &'a str,
        email_verified: bool,
        name: &'a str,
        given_name: &'a str,
        family_name: &'a str,
        iss: &'a str,
        aud: &'a str,
        exp: u64,
    }

    fn test_client() -> GoogleOAuthClient {
        GoogleOAuthClient::new(
            "test-client-id".into(),
            "test-secret".into(),
            "https://example.com/callback".into(),
        )
    }

    fn test_jwks(encoding_key: &EncodingKey) -> JwkSet {
        let mut jwk =
            Jwk::from_encoding_key(encoding_key, Algorithm::RS256).expect("test JWK is valid");
        jwk.common.key_id = Some("test-key".into());

        JwkSet { keys: vec![jwk] }
    }

    fn test_id_token(encoding_key: &EncodingKey, aud: &str, iss: &str) -> String {
        let claims = TestGoogleClaims {
            sub: "google-sub-123",
            email: "user@example.com",
            email_verified: true,
            name: "Test User",
            given_name: "Test",
            family_name: "User",
            iss,
            aud,
            exp: get_current_timestamp() + 300,
        };
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-key".into());

        encode(&header, &claims, encoding_key).expect("test ID token can be encoded")
    }

    #[test]
    fn test_authorization_url_contains_required_params() {
        let client = test_client();

        let url = client.authorization_url("test-state");

        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(url.contains("client_id=test-client-id"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Fexample.com%2Fcallback"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("scope=openid%20email%20profile"));
        assert!(url.contains("state=test-state"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=select_account"));
    }

    #[test]
    fn verify_id_token_accepts_google_issuer_and_client_audience() {
        let client = test_client();
        let crypto = Crypto::for_testing();
        let encoding_key = crypto.encoding_key();
        let jwks = test_jwks(encoding_key);
        let token = test_id_token(
            encoding_key,
            "test-client-id",
            "https://accounts.google.com",
        );

        let claims = client
            .verify_id_token(&token, &jwks)
            .expect("ID token should verify");

        assert_eq!(claims.sub, "google-sub-123");
        assert_eq!(claims.email.as_deref(), Some("user@example.com"));
        assert_eq!(claims.email_verified, Some(true));
    }

    #[test]
    fn verify_id_token_rejects_wrong_audience() {
        let client = test_client();
        let crypto = Crypto::for_testing();
        let encoding_key = crypto.encoding_key();
        let jwks = test_jwks(encoding_key);
        let token = test_id_token(
            encoding_key,
            "other-client-id",
            "https://accounts.google.com",
        );

        let err = client
            .verify_id_token(&token, &jwks)
            .expect_err("wrong audience should fail");

        assert!(err.to_string().contains("Invalid Google ID token"));
    }

    #[test]
    fn verify_id_token_rejects_wrong_issuer() {
        let client = test_client();
        let crypto = Crypto::for_testing();
        let encoding_key = crypto.encoding_key();
        let jwks = test_jwks(encoding_key);
        let token = test_id_token(encoding_key, "test-client-id", "https://attacker.example");

        let err = client
            .verify_id_token(&token, &jwks)
            .expect_err("wrong issuer should fail");

        assert!(err.to_string().contains("Invalid Google ID token"));
    }

    #[test]
    fn verify_id_token_accepts_bare_google_issuer_variant() {
        // Both `https://accounts.google.com` and `accounts.google.com` are valid.
        let client = test_client();
        let crypto = Crypto::for_testing();
        let encoding_key = crypto.encoding_key();
        let jwks = test_jwks(encoding_key);
        let token = test_id_token(encoding_key, "test-client-id", "accounts.google.com");

        let claims = client.verify_id_token(&token, &jwks).unwrap();
        assert_eq!(claims.sub, "google-sub-123");
    }

    #[test]
    fn verify_id_token_rejects_token_without_kid() {
        let client = test_client();
        let crypto = Crypto::for_testing();
        let encoding_key = crypto.encoding_key();
        let jwks = test_jwks(encoding_key);

        // Encode a token whose header omits `kid`.
        let claims = TestGoogleClaims {
            sub: "s",
            email: "u@e.com",
            email_verified: true,
            name: "n",
            given_name: "g",
            family_name: "f",
            iss: "https://accounts.google.com",
            aud: "test-client-id",
            exp: get_current_timestamp() + 300,
        };
        let token = encode(&Header::new(Algorithm::RS256), &claims, encoding_key).unwrap();

        let err = client.verify_id_token(&token, &jwks).unwrap_err();
        assert!(err.to_string().contains("missing key id"));
    }

    #[test]
    fn verify_id_token_rejects_unknown_kid() {
        let client = test_client();
        let crypto = Crypto::for_testing();
        let encoding_key = crypto.encoding_key();
        // JWKS contains "test-key", but the token is signed referencing "other".
        let jwks = test_jwks(encoding_key);

        let claims = TestGoogleClaims {
            sub: "s",
            email: "u@e.com",
            email_verified: true,
            name: "n",
            given_name: "g",
            family_name: "f",
            iss: "https://accounts.google.com",
            aud: "test-client-id",
            exp: get_current_timestamp() + 300,
        };
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("other-key".into());
        let token = encode(&header, &claims, encoding_key).unwrap();

        let err = client.verify_id_token(&token, &jwks).unwrap_err();
        assert!(err.to_string().contains("key id not found"));
    }

    #[test]
    fn verify_id_token_rejects_non_rs256_key() {
        let client = test_client();
        let crypto = Crypto::for_testing();
        let encoding_key = crypto.encoding_key();
        // Build a JWK for "test-key" but mark its algorithm as something other
        // than RS256, so the algorithm guard rejects it before decoding.
        let mut jwk =
            Jwk::from_encoding_key(encoding_key, Algorithm::RS256).expect("test JWK is valid");
        jwk.common.key_id = Some("test-key".into());
        jwk.common.key_algorithm = Some(KeyAlgorithm::RS384);
        let jwks = JwkSet { keys: vec![jwk] };

        let token = test_id_token(
            encoding_key,
            "test-client-id",
            "https://accounts.google.com",
        );
        let err = client.verify_id_token(&token, &jwks).unwrap_err();
        assert!(err.to_string().contains("not RS256"));
    }

    #[test]
    fn verify_id_token_rejects_malformed_token_header() {
        let client = test_client();
        let crypto = Crypto::for_testing();
        let jwks = test_jwks(crypto.encoding_key());
        let err = client.verify_id_token("not-a-jwt", &jwks).unwrap_err();
        assert!(err.to_string().contains("Invalid Google ID token header"));
    }

    #[test]
    fn authorization_url_percent_encodes_state() {
        let client = test_client();
        let url = client.authorization_url("a b&c=d");
        assert!(url.contains("state=a%20b%26c%3Dd"));
    }

    #[tokio::test]
    async fn cached_jwks_returns_fresh_entry_and_jwks_for_kid_hits_cache() {
        let client = test_client();
        let crypto = Crypto::for_testing();
        let jwks = test_jwks(crypto.encoding_key());

        // No cache yet.
        assert!(client.cached_jwks().await.is_none());

        // Seed a fresh cache entry directly.
        *client.jwks_cache.write().await = Some(CachedJwks {
            keys: jwks.clone(),
            fetched_at: Instant::now(),
        });

        // Fresh entry is served.
        assert!(client.cached_jwks().await.is_some());
        // A known kid is served straight from cache (no network fetch).
        let served = client.jwks_for_kid("test-key").await.unwrap();
        assert!(served.find("test-key").is_some());
    }

    #[tokio::test]
    async fn cached_jwks_ignores_stale_entry() {
        let client = test_client();
        let crypto = Crypto::for_testing();
        let jwks = test_jwks(crypto.encoding_key());

        // Seed an entry older than the TTL.
        *client.jwks_cache.write().await = Some(CachedJwks {
            keys: jwks,
            fetched_at: Instant::now()
                .checked_sub(GOOGLE_JWKS_TTL + Duration::from_secs(1))
                .unwrap(),
        });

        // Stale entries are treated as a miss.
        assert!(client.cached_jwks().await.is_none());
    }

    // -------------------------------------------------------------------------
    // Full `exchange_code` coverage over mock token + JWKS endpoints.
    // -------------------------------------------------------------------------
    mod exchange_over_mock {
        use super::*;
        use wiremock::matchers::{method as wm_method, path as wm_path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        fn client_for(token_server: &MockServer, jwks_server: &MockServer) -> GoogleOAuthClient {
            GoogleOAuthClient::with_endpoints(
                "test-client-id".into(),
                "test-secret".into(),
                "https://example.com/callback".into(),
                format!("{}/token", token_server.uri()),
                format!("{}/certs", jwks_server.uri()),
            )
        }

        /// Builds a JWKS containing a single key under `kid`.
        fn jwks_with_kid(encoding_key: &EncodingKey, kid: &str) -> JwkSet {
            let mut jwk =
                Jwk::from_encoding_key(encoding_key, Algorithm::RS256).expect("test JWK is valid");
            jwk.common.key_id = Some(kid.into());
            JwkSet { keys: vec![jwk] }
        }

        /// Signs an ID token referencing `kid` in its header.
        fn id_token_with_kid(encoding_key: &EncodingKey, kid: &str) -> String {
            let claims = TestGoogleClaims {
                sub: "google-sub-123",
                email: "user@example.com",
                email_verified: true,
                name: "Test User",
                given_name: "Test",
                family_name: "User",
                iss: "https://accounts.google.com",
                aud: "test-client-id",
                exp: get_current_timestamp() + 300,
            };
            let mut header = Header::new(Algorithm::RS256);
            header.kid = Some(kid.into());
            encode(&header, &claims, encoding_key).expect("test ID token can be encoded")
        }

        #[tokio::test]
        async fn exchange_code_success_returns_user_info() {
            let crypto = Crypto::for_testing();
            let key = crypto.encoding_key();
            let id_token = id_token_with_kid(key, "test-key");

            let token_server = MockServer::start().await;
            Mock::given(wm_method("POST"))
                .and(wm_path("/token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id_token": id_token,
                    "access_token": "ya29.fake",
                })))
                .mount(&token_server)
                .await;

            let jwks_server = MockServer::start().await;
            Mock::given(wm_method("GET"))
                .and(wm_path("/certs"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(jwks_with_kid(key, "test-key")),
                )
                .mount(&jwks_server)
                .await;

            let info = client_for(&token_server, &jwks_server)
                .exchange_code("auth-code")
                .await
                .expect("exchange should succeed");

            assert_eq!(info.sub, "google-sub-123");
            assert_eq!(info.email.as_deref(), Some("user@example.com"));
            assert!(info.email_verified);
            assert_eq!(info.given_name.as_deref(), Some("Test"));
        }

        #[tokio::test]
        async fn exchange_code_token_endpoint_non_2xx_maps_to_external_service() {
            let token_server = MockServer::start().await;
            Mock::given(wm_method("POST"))
                .and(wm_path("/token"))
                .respond_with(ResponseTemplate::new(400).set_body_string("invalid_grant"))
                .mount(&token_server)
                .await;
            // JWKS server unused here.
            let jwks_server = MockServer::start().await;

            let err = client_for(&token_server, &jwks_server)
                .exchange_code("bad-code")
                .await
                .unwrap_err();
            assert!(matches!(err, DomainError::ExternalService(_)));
            assert!(err.to_string().contains("token exchange failed"));
        }

        #[tokio::test]
        async fn exchange_code_token_parse_error_maps_to_external_service() {
            // 2xx but the body is missing the required token fields.
            let token_server = MockServer::start().await;
            Mock::given(wm_method("POST"))
                .and(wm_path("/token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "unexpected": "shape"
                })))
                .mount(&token_server)
                .await;
            let jwks_server = MockServer::start().await;

            let err = client_for(&token_server, &jwks_server)
                .exchange_code("code")
                .await
                .unwrap_err();
            assert!(matches!(err, DomainError::ExternalService(_)));
            assert!(err.to_string().contains("parse Google token response"));
        }

        #[tokio::test]
        async fn exchange_code_unknown_kid_triggers_jwks_refetch() {
            // A stale cache holds only "old-key", but the freshly issued token is
            // signed under "new-key" (a key rotation). The unknown kid must force
            // a fresh JWKS fetch from the endpoint, after which verification
            // succeeds.
            let crypto = Crypto::for_testing();
            let key = crypto.encoding_key();
            let id_token = id_token_with_kid(key, "new-key");

            let token_server = MockServer::start().await;
            Mock::given(wm_method("POST"))
                .and(wm_path("/token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id_token": id_token,
                    "access_token": "ya29.fake",
                })))
                .mount(&token_server)
                .await;

            let jwks_server = MockServer::start().await;
            // Endpoint serves the rotated JWKS containing "new-key". Expect it to
            // be hit exactly once because of the refetch.
            Mock::given(wm_method("GET"))
                .and(wm_path("/certs"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(jwks_with_kid(key, "new-key")),
                )
                .expect(1)
                .mount(&jwks_server)
                .await;

            let client = client_for(&token_server, &jwks_server);
            // Seed a fresh-but-stale-by-kid cache entry that lacks "new-key".
            *client.jwks_cache.write().await = Some(CachedJwks {
                keys: jwks_with_kid(key, "old-key"),
                fetched_at: Instant::now(),
            });

            let info = client
                .exchange_code("code")
                .await
                .expect("refetch should resolve the rotated key");
            assert_eq!(info.sub, "google-sub-123");
            // wiremock verifies the .expect(1) on drop.
        }
    }
}
