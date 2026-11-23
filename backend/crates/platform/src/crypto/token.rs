//! Centralized JWT envelope rules (issuer, audience, timestamps, and token
//! type) layered on top of [`TokenKeyring`].
//!
//! Every application JWT minted with the shared [`TokenKeyring`] — session,
//! signup, password-reset, and OAuth-state — flows through [`issue`] and
//! [`verify`], so the `typ`, `iss`, `aud`, `iat`, and `exp` invariants live in
//! one place instead of being duplicated across adapters.

use std::fmt;
use std::sync::LazyLock;

use jiff::Timestamp;
use jsonwebtoken::Validation;
use jsonwebtoken::errors::ErrorKind;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::keyring::TokenKeyring;

/// Issuer (`iss`) stamped on every application JWT and required on verify.
///
/// Combined with [`TOKEN_AUDIENCE`], this scopes tokens to this service so a
/// token minted by or for another service — even sharing a signing key — is
/// rejected.
pub const TOKEN_ISSUER: &str = "overkill-todo";

/// Audience (`aud`) stamped on every application JWT and required on verify.
pub const TOKEN_AUDIENCE: &str = "overkill-todo-api";

/// Purpose of an application JWT minted and verified through [`TokenKeyring`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    /// Authenticated API session token.
    Session,
    /// Single-purpose email-verification / signup completion token.
    Signup,
    /// Single-purpose password-reset authorization token.
    Reset,
    /// Short-lived OAuth CSRF / redirect state token.
    OAuthState,
}

impl TokenType {
    /// Canonical `typ` claim value embedded in tokens of this kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Signup => "signup",
            Self::Reset => "reset",
            Self::OAuthState => "oauth_state",
        }
    }

    const fn requires_sub(self) -> bool {
        match self {
            Self::Session | Self::Signup | Self::Reset => true,
            Self::OAuthState => false,
        }
    }

    fn validation(self) -> &'static Validation {
        if self.requires_sub() {
            &USER_TOKEN_VALIDATION
        } else {
            &STATE_TOKEN_VALIDATION
        }
    }
}

impl fmt::Display for TokenType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Verified token envelope alongside its deserialized domain payload `P`.
#[derive(Debug)]
pub struct VerifiedToken<P> {
    pub typ: TokenType,
    pub iss: String,
    pub aud: String,
    pub iat: i64,
    pub exp: i64,
    pub payload: P,
}

/// Failure modes when verifying an application JWT.
#[derive(Debug, thiserror::Error)]
pub enum TokenVerifyError {
    /// Signature and claims are structurally valid, but `exp` is in the past.
    #[error("token expired")]
    Expired,
    /// Signature and envelope checks passed, but `typ` did not match the caller's expectation.
    #[error("wrong token type: expected {expected}, got {actual}")]
    WrongType { expected: TokenType, actual: String },
    /// Signature, key ID, algorithm, `iss`, `aud`, required claims, or decoding failed.
    #[error("invalid token: {0}")]
    Invalid(#[from] jsonwebtoken::errors::Error),
}

static USER_TOKEN_VALIDATION: LazyLock<Validation> = LazyLock::new(|| {
    let mut validation = TokenKeyring::validation();
    validation.set_issuer(&[TOKEN_ISSUER]);
    validation.set_audience(&[TOKEN_AUDIENCE]);
    validation.set_required_spec_claims(&["sub", "exp", "iss", "aud"]);
    validation
});

static STATE_TOKEN_VALIDATION: LazyLock<Validation> = LazyLock::new(|| {
    let mut validation = TokenKeyring::validation();
    validation.set_issuer(&[TOKEN_ISSUER]);
    validation.set_audience(&[TOKEN_AUDIENCE]);
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);
    validation
});

#[derive(Serialize)]
struct OutgoingEnvelope<'a, P> {
    typ: &'static str,
    iss: &'static str,
    aud: &'static str,
    iat: i64,
    exp: i64,
    #[serde(flatten)]
    payload: &'a P,
}

#[derive(Deserialize)]
struct IncomingEnvelope<P> {
    typ: String,
    iss: String,
    aud: String,
    iat: i64,
    exp: i64,
    #[serde(flatten)]
    payload: P,
}

/// Stamp `typ`, `iss`, `aud`, `iat`, and `exp` around `payload` and sign with
/// the keyring's active HMAC-SHA-512 key.
///
/// # Errors
///
/// Returns the underlying encoder error if serialization or signing fails.
pub fn issue<P: Serialize>(
    keys: &TokenKeyring,
    typ: TokenType,
    payload: &P,
    iat: Timestamp,
    exp: Timestamp,
) -> Result<String, jsonwebtoken::errors::Error> {
    let envelope = OutgoingEnvelope {
        typ: typ.as_str(),
        iss: TOKEN_ISSUER,
        aud: TOKEN_AUDIENCE,
        iat: iat.as_second(),
        exp: exp.as_second(),
        payload,
    };
    keys.sign(&envelope)
}

/// Verify an application JWT's signature, algorithm, key ID, `exp`, `iss`,
/// `aud`, required claims (`sub` for user-bound token types), and `typ`.
///
/// # Errors
///
/// Returns [`TokenVerifyError::Expired`] if `exp` is past the leeway window,
/// [`TokenVerifyError::WrongType`] if `typ` does not match `expected`, or
/// [`TokenVerifyError::Invalid`] for any other cryptographic or claim failure.
pub fn verify<P: DeserializeOwned>(
    keys: &TokenKeyring,
    token: &str,
    expected: TokenType,
) -> Result<VerifiedToken<P>, TokenVerifyError> {
    let envelope: IncomingEnvelope<P> =
        keys.verify(token, expected.validation()).map_err(|err| {
            if *err.kind() == ErrorKind::ExpiredSignature {
                TokenVerifyError::Expired
            } else {
                TokenVerifyError::Invalid(err)
            }
        })?;

    if envelope.typ != expected.as_str() {
        return Err(TokenVerifyError::WrongType {
            expected,
            actual: envelope.typ,
        });
    }

    Ok(VerifiedToken {
        typ: expected,
        iss: envelope.iss,
        aud: envelope.aud,
        iat: envelope.iat,
        exp: envelope.exp,
        payload: envelope.payload,
    })
}

#[cfg(test)]
mod tests {
    use jiff::SignedDuration;

    use super::*;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct SubPayload {
        sub: String,
    }

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct StatePayload {
        nonce: String,
    }

    fn keys() -> TokenKeyring {
        TokenKeyring::for_testing()
    }

    fn future_window() -> (Timestamp, Timestamp) {
        let now = Timestamp::now();
        (now, now + SignedDuration::from_mins(15))
    }

    #[test]
    fn round_trips_all_token_types() {
        let k = keys();
        let (iat, exp) = future_window();

        for typ in [TokenType::Session, TokenType::Signup, TokenType::Reset] {
            let payload = SubPayload {
                sub: "user-123".into(),
            };
            let jwt = issue(&k, typ, &payload, iat, exp).unwrap();
            let verified: VerifiedToken<SubPayload> = verify(&k, &jwt, typ).unwrap();
            assert_eq!(verified.typ, typ);
            assert_eq!(verified.iss, TOKEN_ISSUER);
            assert_eq!(verified.aud, TOKEN_AUDIENCE);
            assert_eq!(verified.iat, iat.as_second());
            assert_eq!(verified.exp, exp.as_second());
            assert_eq!(verified.payload, payload);
        }

        let state = StatePayload {
            nonce: "csrf-nonce".into(),
        };
        let state_jwt = issue(&k, TokenType::OAuthState, &state, iat, exp).unwrap();
        let verified_state: VerifiedToken<StatePayload> =
            verify(&k, &state_jwt, TokenType::OAuthState).unwrap();
        assert_eq!(verified_state.typ, TokenType::OAuthState);
        assert_eq!(verified_state.iss, TOKEN_ISSUER);
        assert_eq!(verified_state.aud, TOKEN_AUDIENCE);
        assert_eq!(verified_state.payload, state);
    }

    #[test]
    fn cross_type_presentation_is_rejected() {
        let k = keys();
        let (iat, exp) = future_window();
        let sub_payload = SubPayload {
            sub: "user-123".into(),
        };
        let state_payload = StatePayload {
            nonce: "nonce-1".into(),
        };

        let all = [
            (
                TokenType::Session,
                issue(&k, TokenType::Session, &sub_payload, iat, exp).unwrap(),
            ),
            (
                TokenType::Signup,
                issue(&k, TokenType::Signup, &sub_payload, iat, exp).unwrap(),
            ),
            (
                TokenType::Reset,
                issue(&k, TokenType::Reset, &sub_payload, iat, exp).unwrap(),
            ),
            (
                TokenType::OAuthState,
                issue(&k, TokenType::OAuthState, &state_payload, iat, exp).unwrap(),
            ),
        ];

        for (actual_typ, tok) in &all {
            for expected_typ in [
                TokenType::Session,
                TokenType::Signup,
                TokenType::Reset,
                TokenType::OAuthState,
            ] {
                if *actual_typ == expected_typ {
                    continue;
                }
                let err = verify::<serde_json::Value>(&k, tok, expected_typ).unwrap_err();
                // OAuthState carries no `sub`, so user-bound validators reject it
                // at `USER_TOKEN_VALIDATION` before inspecting `typ`; either
                // `WrongType` or `Invalid` confirms rejection.
                match (*actual_typ, err) {
                    (TokenType::OAuthState, TokenVerifyError::Invalid(_)) => {}
                    (_, TokenVerifyError::WrongType { expected, actual }) => {
                        assert_eq!(expected, expected_typ);
                        assert_eq!(actual, actual_typ.as_str());
                    }
                    (other_typ, other_err) => panic!(
                        "unexpected error for {other_typ} presented as {expected_typ}: {other_err:?}"
                    ),
                }
            }
        }
    }

    #[derive(Serialize)]
    struct RawEnvelope<'a> {
        typ: &'static str,
        iss: &'a str,
        aud: &'a str,
        iat: i64,
        exp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        sub: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        nonce: Option<&'a str>,
    }

    #[test]
    fn foreign_audience_is_rejected_for_all_types() {
        let k = keys();
        let (iat, exp) = future_window();

        for typ in [
            TokenType::Session,
            TokenType::Signup,
            TokenType::Reset,
            TokenType::OAuthState,
        ] {
            let jwt = k
                .sign(&RawEnvelope {
                    typ: typ.as_str(),
                    iss: TOKEN_ISSUER,
                    aud: "other-audience",
                    iat: iat.as_second(),
                    exp: exp.as_second(),
                    sub: (typ != TokenType::OAuthState).then_some("user-1"),
                    nonce: (typ == TokenType::OAuthState).then_some("n"),
                })
                .unwrap();
            let err = verify::<serde_json::Value>(&k, &jwt, typ).unwrap_err();
            assert!(
                matches!(err, TokenVerifyError::Invalid(_)),
                "expected foreign aud rejection for {typ}, got {err:?}"
            );
        }
    }

    #[test]
    fn foreign_issuer_is_rejected_for_all_types() {
        let k = keys();
        let (iat, exp) = future_window();

        for typ in [
            TokenType::Session,
            TokenType::Signup,
            TokenType::Reset,
            TokenType::OAuthState,
        ] {
            let jwt = k
                .sign(&RawEnvelope {
                    typ: typ.as_str(),
                    iss: "other-issuer",
                    aud: TOKEN_AUDIENCE,
                    iat: iat.as_second(),
                    exp: exp.as_second(),
                    sub: (typ != TokenType::OAuthState).then_some("user-1"),
                    nonce: (typ == TokenType::OAuthState).then_some("n"),
                })
                .unwrap();
            let err = verify::<serde_json::Value>(&k, &jwt, typ).unwrap_err();
            assert!(
                matches!(err, TokenVerifyError::Invalid(_)),
                "expected foreign iss rejection for {typ}, got {err:?}"
            );
        }
    }

    #[test]
    fn missing_sub_rejected_for_user_tokens() {
        let k = keys();
        let (iat, exp) = future_window();

        for typ in [TokenType::Session, TokenType::Signup, TokenType::Reset] {
            let jwt = k
                .sign(&RawEnvelope {
                    typ: typ.as_str(),
                    iss: TOKEN_ISSUER,
                    aud: TOKEN_AUDIENCE,
                    iat: iat.as_second(),
                    exp: exp.as_second(),
                    sub: None,
                    nonce: None,
                })
                .unwrap();
            let err = verify::<serde_json::Value>(&k, &jwt, typ).unwrap_err();
            assert!(
                matches!(err, TokenVerifyError::Invalid(_)),
                "expected missing sub rejection for {typ}, got {err:?}"
            );
        }
    }

    #[test]
    fn expired_token_returns_expired() {
        let k = keys();
        let past = Timestamp::now() - SignedDuration::from_mins(5);
        let iat = past - SignedDuration::from_mins(15);
        let jwt = issue(
            &k,
            TokenType::Session,
            &SubPayload {
                sub: "user-1".into(),
            },
            iat,
            past,
        )
        .unwrap();

        let err = verify::<SubPayload>(&k, &jwt, TokenType::Session).unwrap_err();
        assert!(matches!(err, TokenVerifyError::Expired));
    }

    #[test]
    fn missing_typ_is_rejected() {
        #[derive(Serialize)]
        struct NoTyp {
            iss: &'static str,
            aud: &'static str,
            sub: &'static str,
            iat: i64,
            exp: i64,
        }

        let k = keys();
        let (iat, exp) = future_window();
        let jwt = k
            .sign(&NoTyp {
                iss: TOKEN_ISSUER,
                aud: TOKEN_AUDIENCE,
                sub: "user-1",
                iat: iat.as_second(),
                exp: exp.as_second(),
            })
            .unwrap();

        let err = verify::<serde_json::Value>(&k, &jwt, TokenType::Session).unwrap_err();
        assert!(matches!(err, TokenVerifyError::Invalid(_)));
    }
}
