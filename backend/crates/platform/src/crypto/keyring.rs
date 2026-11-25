//! Named HMAC-SHA-512 keys for every token the backend issues and verifies.
//!
//! Session, signup, password-reset and OAuth-state tokens are only ever
//! verified by this backend, so they're signed with a symmetric MAC rather
//! than a public-key signature. HMAC-SHA-512 with a key of at least 64 bytes
//! is quantum-safe (Grover's algorithm at most halves the effective key
//! length), unlike the RS256 signatures it replaces. See OT-009 design D1.
//!
//! Every token names its key in the JWS `kid` header. One key is active (it
//! signs and verifies). Others may be trusted for verification only, so a key
//! can be rotated without signing anyone out (OT-009 design D5). With HMAC a
//! verification key can also sign, so every trusted key is a secret.

use std::collections::HashMap;
use std::fmt;

use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::decode_secret_b64;

/// The only algorithm tokens are signed with or accepted under.
pub const TOKEN_ALGORITHM: Algorithm = Algorithm::HS512;

/// Minimum decoded length of a token key: the HMAC-SHA-512 block-size-safe
/// floor that keeps 256-bit security against a quantum attacker.
pub const MIN_TOKEN_KEY_LEN: usize = 64;

/// Maximum length of a key name (`kid`).
pub const MAX_KID_LEN: usize = 16;

/// The message for every rejected token except an expired session token,
/// whatever kind of token it was and whichever check failed, so a client
/// learns nothing about why.
pub const TOKEN_REJECTED_MESSAGE: &str = "Invalid token";

/// The setting holding the active key.
pub const SIGNING_KEY_SETTING: &str = "JWT_SIGNING_KEY";
/// The setting holding previous keys still trusted for verification.
pub const TRUSTED_KEYS_SETTING: &str = "JWT_TRUSTED_KEYS";

/// A misconfigured token key. Messages name the setting and, once it has been
/// validated as a well-formed name, the key name. They never include key bytes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyringError {
    /// `JWT_SIGNING_KEY` is empty.
    #[error(
        "{SIGNING_KEY_SETTING} is empty: set it to `<kid>:<base64 secret>` (generate one with `just jwt-keygen`)"
    )]
    NoActiveKey,
    /// An entry isn't `<kid>:<secret>`.
    #[error("{setting}: entry {index} is not `<kid>:<base64 secret>`")]
    Shape {
        /// The setting the entry came from.
        setting: &'static str,
        /// 1-based position of the entry in the setting.
        index: usize,
    },
    /// A key name is empty, too long, or uses characters outside the allowed
    /// set. The name itself isn't echoed, in case a secret was pasted there.
    #[error(
        "{setting}: entry {index} has a malformed key name (must be 1–{MAX_KID_LEN} characters from A-Z, a-z, 0-9, '_' and '-')"
    )]
    BadKid {
        /// The setting the entry came from.
        setting: &'static str,
        /// 1-based position of the entry in the setting.
        index: usize,
    },
    /// A key's secret isn't valid base64.
    #[error("{setting}: key {kid:?} is not valid base64 (url-safe or standard)")]
    Encoding {
        /// The setting the key came from.
        setting: &'static str,
        /// The key's name.
        kid: String,
    },
    /// A key's secret is too short for HMAC-SHA-512.
    #[error(
        "{setting}: key {kid:?} decodes to {len} bytes; token keys must be at least {MIN_TOKEN_KEY_LEN}"
    )]
    TooShort {
        /// The setting the key came from.
        setting: &'static str,
        /// The key's name.
        kid: String,
        /// Decoded length in bytes.
        len: usize,
    },
    /// Two keys share a name across both settings.
    #[error(
        "key name {kid:?} is used more than once across {SIGNING_KEY_SETTING} and {TRUSTED_KEYS_SETTING}"
    )]
    DuplicateKid {
        /// The repeated name.
        kid: String,
    },
}

fn is_valid_kid(kid: &str) -> bool {
    (1..=MAX_KID_LEN).contains(&kid.len())
        && kid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Parse one `<kid>:<base64 secret>` entry into its name and secret bytes.
fn parse_entry(
    setting: &'static str,
    index: usize,
    entry: &str,
) -> Result<(String, Vec<u8>), KeyringError> {
    let (kid, secret) = entry
        .trim()
        .split_once(':')
        .ok_or(KeyringError::Shape { setting, index })?;
    if !is_valid_kid(kid) {
        return Err(KeyringError::BadKid { setting, index });
    }
    let kid = kid.to_owned();
    let bytes = decode_secret_b64(secret).map_err(|_| KeyringError::Encoding {
        setting,
        kid: kid.clone(),
    })?;
    if bytes.len() < MIN_TOKEN_KEY_LEN {
        return Err(too_short(setting, &kid, &bytes));
    }
    Ok((kid, bytes))
}

fn too_short(setting: &'static str, kid: &str, secret: &[u8]) -> KeyringError {
    KeyringError::TooShort {
        setting,
        kid: kid.to_owned(),
        len: secret.len(),
    }
}

/// Build a verification key. jsonwebtoken refuses only an empty secret, which
/// [`parse_entry`] has already ruled out, so a failure is reported as too short.
fn decoding_key(
    setting: &'static str,
    kid: &str,
    secret: &[u8],
) -> Result<DecodingKey, KeyringError> {
    DecodingKey::from_secret(secret).map_err(|_| too_short(setting, kid, secret))
}

/// The active signing key plus every key trusted for verification.
pub struct TokenKeyring {
    active_kid: String,
    active: EncodingKey,
    /// Every trusted key by name, including the active one.
    trusted: HashMap<String, DecodingKey>,
}

impl fmt::Debug for TokenKeyring {
    // Names only: the keys are secrets.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenKeyring")
            .field("active_kid", &self.active_kid)
            .field("trusted_kids", &self.trusted_kids())
            .finish_non_exhaustive()
    }
}

impl TokenKeyring {
    /// Build from the raw `JWT_SIGNING_KEY` and optional `JWT_TRUSTED_KEYS`
    /// values (`<kid>:<base64 secret>`, the latter comma-separated).
    ///
    /// # Errors
    ///
    /// Returns a [`KeyringError`] naming the first misconfiguration found.
    pub fn from_config(signing: &str, trusted: Option<&str>) -> Result<Self, KeyringError> {
        if signing.trim().is_empty() {
            return Err(KeyringError::NoActiveKey);
        }
        let (active_kid, active_secret) = parse_entry(SIGNING_KEY_SETTING, 1, signing)?;

        let mut keys = HashMap::new();
        keys.insert(
            active_kid.clone(),
            decoding_key(SIGNING_KEY_SETTING, &active_kid, &active_secret)?,
        );
        // Number entries before skipping empty ones, so an error's position
        // matches the comma-separated item the operator sees.
        let entries = trusted
            .unwrap_or_default()
            .split(',')
            .enumerate()
            .filter(|(_, e)| !e.trim().is_empty());
        for (i, entry) in entries {
            let (kid, secret) = parse_entry(TRUSTED_KEYS_SETTING, i + 1, entry)?;
            if keys.contains_key(&kid) {
                return Err(KeyringError::DuplicateKid { kid });
            }
            let key = decoding_key(TRUSTED_KEYS_SETTING, &kid, &secret)?;
            keys.insert(kid, key);
        }

        Ok(Self {
            active: EncodingKey::from_secret(&active_secret)
                .map_err(|_| too_short(SIGNING_KEY_SETTING, &active_kid, &active_secret))?,
            active_kid,
            trusted: keys,
        })
    }

    /// The name of the key new tokens are signed with.
    #[must_use]
    pub fn active_kid(&self) -> &str {
        &self.active_kid
    }

    /// Every trusted key name, active one included, sorted.
    #[must_use]
    pub fn trusted_kids(&self) -> Vec<&str> {
        let mut kids: Vec<&str> = self.trusted.keys().map(String::as_str).collect();
        kids.sort_unstable();
        kids
    }

    /// Log which keys are loaded: names only, never secrets. Called once at
    /// startup so a rotation can be confirmed from the logs.
    pub fn log_configuration(&self) {
        tracing::info!(
            active_kid = %self.active_kid,
            trusted_kids = %self.trusted_kids().join(","),
            "token keys loaded"
        );
    }

    /// A [`Validation`] for [`TokenKeyring::verify`], pinned to
    /// [`TOKEN_ALGORITHM`]. Callers add their issuer, audience and
    /// required-claim checks, and should build it once (e.g. in a
    /// `LazyLock`) rather than per request.
    #[must_use]
    pub fn validation() -> Validation {
        Validation::new(TOKEN_ALGORITHM)
    }

    /// Sign `claims` with the active key, naming it in the `kid` header.
    ///
    /// # Errors
    ///
    /// Returns the encoder's error if the claims can't be serialised.
    pub fn sign<T: Serialize>(&self, claims: &T) -> Result<String, jsonwebtoken::errors::Error> {
        let mut header = Header::new(TOKEN_ALGORITHM);
        header.kid = Some(self.active_kid.clone());
        encode(&header, claims, &self.active)
    }

    /// Verify `token` with the key its `kid` names and return its claims.
    ///
    /// A token naming no key, or a key that isn't trusted, is refused without
    /// trying any other key. `validation` must come from
    /// [`TokenKeyring::validation`]: one allowing any other algorithm is
    /// refused outright, so only [`TOKEN_ALGORITHM`] tokens can ever verify and
    /// an RS256, HS256 or `none` token is refused even if otherwise valid.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::InvalidToken`] for a missing or unknown `kid`, and
    /// otherwise the decoder's error (expired, bad signature, wrong algorithm,
    /// failed claim check, malformed token).
    pub fn verify<T: DeserializeOwned>(
        &self,
        token: &str,
        validation: &Validation,
    ) -> Result<T, jsonwebtoken::errors::Error> {
        if validation.algorithms != [TOKEN_ALGORITHM] {
            return Err(ErrorKind::InvalidAlgorithm.into());
        }
        let header = decode_header(token)?;
        let key = header
            .kid
            .as_deref()
            .and_then(|kid| self.trusted.get(kid))
            .ok_or_else(|| jsonwebtoken::errors::Error::from(ErrorKind::InvalidToken))?;
        decode::<T>(token, key, validation).map(|data| data.claims)
    }

    /// A keyring with fixed, test-only keys: active `test-a`, also trusting
    /// `test-b`. Never use in production.
    ///
    /// # Panics
    ///
    /// Never: the fixed test keys are well-formed.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn for_testing() -> Self {
        Self::from_config(&test_entry("test-a", 1), Some(&test_entry("test-b", 2)))
            .expect("test keys are valid")
    }
}

/// A `<kid>:<base64 secret>` entry with a fixed 64-byte test secret derived from
/// `fill`. Test use only.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn test_entry(kid: &str, fill: u8) -> String {
    use base64::Engine;
    format!(
        "{kid}:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([fill; MIN_TOKEN_KEY_LEN])
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    struct TestClaims {
        sub: String,
        exp: i64,
    }

    fn claims() -> TestClaims {
        TestClaims {
            sub: "user-1".into(),
            exp: jiff::Timestamp::now().as_second() + 600,
        }
    }

    fn verify(ring: &TokenKeyring, token: &str) -> Result<TestClaims, jsonwebtoken::errors::Error> {
        ring.verify(token, &TokenKeyring::validation())
    }

    /// Sign with an arbitrary header and key, bypassing the keyring.
    fn forge(header: &Header, key: &EncodingKey) -> String {
        encode(header, &claims(), key).expect("encodes")
    }

    fn hs512_with_kid(kid: Option<&str>) -> Header {
        let mut h = Header::new(Algorithm::HS512);
        h.kid = kid.map(str::to_owned);
        h
    }

    fn secret(fill: u8) -> [u8; MIN_TOKEN_KEY_LEN] {
        [fill; MIN_TOKEN_KEY_LEN]
    }

    #[test]
    fn round_trips_and_names_the_active_key() {
        let ring = TokenKeyring::for_testing();
        let token = ring.sign(&claims()).unwrap();
        let header = decode_header(&token).unwrap();
        assert_eq!(header.alg, Algorithm::HS512);
        assert_eq!(header.kid.as_deref(), Some("test-a"));
        assert_eq!(verify(&ring, &token).unwrap().sub, "user-1");
    }

    #[test]
    fn accepts_a_token_from_a_trusted_previous_key() {
        let ring = TokenKeyring::for_testing();
        let token = forge(
            &hs512_with_kid(Some("test-b")),
            &EncodingKey::from_secret(&secret(2)).unwrap(),
        );
        assert!(verify(&ring, &token).is_ok());
    }

    #[test]
    fn refuses_a_token_from_a_retired_key() {
        // `test-b` signed this, but the new ring no longer trusts it.
        let old = TokenKeyring::from_config(&test_entry("test-b", 2), None).unwrap();
        let token = old.sign(&claims()).unwrap();
        let ring = TokenKeyring::from_config(&test_entry("test-a", 1), None).unwrap();
        assert_eq!(
            verify(&ring, &token).unwrap_err().kind(),
            &ErrorKind::InvalidToken
        );
    }

    #[test]
    fn refuses_a_missing_or_unknown_kid_without_trying_other_keys() {
        let ring = TokenKeyring::for_testing();
        let active = EncodingKey::from_secret(&secret(1)).unwrap();
        // Correctly signed by the active key, but naming no key or another one.
        for kid in [None, Some("nope"), Some("test-b")] {
            let token = forge(&hs512_with_kid(kid), &active);
            assert!(
                verify(&ring, &token).is_err(),
                "kid {kid:?} must be refused"
            );
        }
    }

    #[test]
    fn refuses_hs256_under_the_right_key() {
        let ring = TokenKeyring::for_testing();
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("test-a".into());
        let token = forge(&header, &EncodingKey::from_secret(&secret(1)).unwrap());
        assert_eq!(
            verify(&ring, &token).unwrap_err().kind(),
            &ErrorKind::InvalidAlgorithm
        );
    }

    #[test]
    fn refuses_any_validation_allowing_another_algorithm() {
        let ring = TokenKeyring::for_testing();
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-a".into());
        let token = forge(&header, &test_rsa_key());
        let mut lax = TokenKeyring::validation();
        lax.algorithms = vec![Algorithm::RS256, Algorithm::HS512];
        assert_eq!(
            ring.verify::<TestClaims>(&token, &lax).unwrap_err().kind(),
            &ErrorKind::InvalidAlgorithm
        );
        // Even our own valid token is refused under a lax validation.
        let ours = ring.sign(&claims()).unwrap();
        assert!(ring.verify::<TestClaims>(&ours, &lax).is_err());
    }

    #[test]
    fn refuses_alg_none() {
        let ring = TokenKeyring::for_testing();
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","kid":"test-a","typ":"JWT"}"#);
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims()).unwrap());
        for token in [format!("{header}.{body}."), format!("{header}.{body}")] {
            assert!(verify(&ring, &token).is_err());
        }
    }

    #[test]
    fn refuses_any_change_to_header_claims_or_signature() {
        let ring = TokenKeyring::for_testing();
        let token = ring.sign(&claims()).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        // Re-encoded header and claims that still parse, plus a flipped signature.
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS512","kid":"test-a","typ":"jwt"}"#);
        let mut other = claims();
        other.sub = "user-2".into();
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&other).unwrap());
        let mut sig = parts[2].to_owned();
        let last = sig.pop().unwrap();
        sig.push(if last == 'A' { 'B' } else { 'A' });
        for tampered in [
            format!("{header}.{}.{}", parts[1], parts[2]),
            format!("{}.{body}.{}", parts[0], parts[2]),
            format!("{}.{}.{sig}", parts[0], parts[1]),
        ] {
            assert!(verify(&ring, &tampered).is_err(), "{tampered}");
        }
    }

    #[test]
    fn refuses_malformed_tokens() {
        let ring = TokenKeyring::for_testing();
        for token in ["", "only-one-segment", "a.b", "a.b.c.d", "!!!.###.$$$"] {
            assert!(verify(&ring, token).is_err(), "{token:?}");
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one flat table of every misconfiguration case; splitting it hides which are covered"
    )]
    fn rejects_every_misconfiguration() {
        let good = test_entry("ok", 1);
        let short = format!("ok:{}", URL_SAFE_NO_PAD.encode([1u8; 63]));
        let cases: Vec<(String, Option<String>, KeyringError)> = vec![
            ("  ".into(), None, KeyringError::NoActiveKey),
            (
                "no-colon".into(),
                None,
                KeyringError::Shape {
                    setting: SIGNING_KEY_SETTING,
                    index: 1,
                },
            ),
            (
                format!(":{}", URL_SAFE_NO_PAD.encode(secret(1))),
                None,
                KeyringError::BadKid {
                    setting: SIGNING_KEY_SETTING,
                    index: 1,
                },
            ),
            (
                format!(
                    "this-name-is-too-long:{}",
                    URL_SAFE_NO_PAD.encode(secret(1))
                ),
                None,
                KeyringError::BadKid {
                    setting: SIGNING_KEY_SETTING,
                    index: 1,
                },
            ),
            (
                format!("bad kid:{}", URL_SAFE_NO_PAD.encode(secret(1))),
                None,
                KeyringError::BadKid {
                    setting: SIGNING_KEY_SETTING,
                    index: 1,
                },
            ),
            (
                "ok:!!!not base64!!!".into(),
                None,
                KeyringError::Encoding {
                    setting: SIGNING_KEY_SETTING,
                    kid: "ok".into(),
                },
            ),
            (
                short.clone(),
                None,
                KeyringError::TooShort {
                    setting: SIGNING_KEY_SETTING,
                    kid: "ok".into(),
                    len: 63,
                },
            ),
            (
                good.clone(),
                Some(format!("{},garbage", test_entry("b", 2))),
                KeyringError::Shape {
                    setting: TRUSTED_KEYS_SETTING,
                    index: 2,
                },
            ),
            (
                good.clone(),
                Some(format!("{},,garbage", test_entry("b", 2))),
                KeyringError::Shape {
                    setting: TRUSTED_KEYS_SETTING,
                    index: 3,
                },
            ),
            (
                good.clone(),
                Some(short),
                KeyringError::TooShort {
                    setting: TRUSTED_KEYS_SETTING,
                    kid: "ok".into(),
                    len: 63,
                },
            ),
            (
                good.clone(),
                Some(test_entry("ok", 2)),
                KeyringError::DuplicateKid { kid: "ok".into() },
            ),
            (
                good,
                Some(format!("{},{}", test_entry("b", 2), test_entry("b", 3))),
                KeyringError::DuplicateKid { kid: "b".into() },
            ),
        ];
        for (signing, trusted, want) in cases {
            let err = TokenKeyring::from_config(&signing, trusted.as_deref()).unwrap_err();
            assert_eq!(err, want, "signing={signing:?} trusted={trusted:?}");
            // Messages never carry key material.
            let msg = err.to_string();
            for secret in signing
                .split(':')
                .skip(1)
                .chain(trusted.iter().flat_map(|t| t.split([':', ','])))
            {
                if secret.len() > MAX_KID_LEN {
                    assert!(!msg.contains(secret), "message leaks a secret: {msg}");
                }
            }
        }
    }

    #[test]
    fn accepts_standard_base64_and_ignores_empty_trusted_entries() {
        let std_b64 = format!(
            "k1:{}",
            base64::engine::general_purpose::STANDARD.encode(secret(9))
        );
        let ring = TokenKeyring::from_config(&std_b64, Some(" , ")).unwrap();
        assert_eq!(ring.trusted_kids(), vec!["k1"]);
    }

    #[test]
    fn debug_shows_names_only() {
        let dbg = format!("{:?}", TokenKeyring::for_testing());
        assert!(dbg.contains("test-a") && dbg.contains("test-b"));
        assert!(!dbg.contains(&URL_SAFE_NO_PAD.encode(secret(1))));
    }

    #[test]
    fn startup_log_names_the_keys_and_holds_no_secrets() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
            type Writer = Self;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            TokenKeyring::for_testing().log_configuration();
        });

        let out = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert!(out.contains("token keys loaded"), "{out}");
        assert!(out.contains("active_kid=test-a"), "{out}");
        assert!(out.contains("trusted_kids=test-a,test-b"), "{out}");
        for fill in [1u8, 2] {
            let secret = URL_SAFE_NO_PAD.encode(secret(fill));
            assert!(!out.contains(&secret), "log leaks a key: {out}");
        }
    }

    fn test_rsa_key() -> EncodingKey {
        EncodingKey::from_rsa_pem(super::super::TEST_RSA_PRIVATE_KEY.as_bytes())
            .expect("test RSA key is valid")
    }

    proptest::proptest! {
        /// Arbitrary input never panics and never verifies.
        #[test]
        fn prop_random_input_never_verifies(token in "\\PC{0,400}") {
            let ring = TokenKeyring::for_testing();
            proptest::prop_assert!(verify(&ring, &token).is_err());
        }

        /// Random three-segment base64url input never verifies.
        #[test]
        fn prop_random_segments_never_verify(
            a in "[A-Za-z0-9_-]{0,80}", b in "[A-Za-z0-9_-]{0,80}", c in "[A-Za-z0-9_-]{0,120}"
        ) {
            let ring = TokenKeyring::for_testing();
            let token = format!("{a}.{b}.{c}");
            proptest::prop_assert!(verify(&ring, &token).is_err());
        }
    }
}
