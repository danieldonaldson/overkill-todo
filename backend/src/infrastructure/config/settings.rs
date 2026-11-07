//! Application settings loaded from environment variables.
//!
//! All required variables must be set — the application panics at startup
//! with a clear error listing ALL missing variables.
//!
//! Settings are grouped into per-concern sub-structs ([`ServerSettings`],
//! [`SmtpSettings`], [`CryptoSettings`], …) rather than one flat god-struct,
//! so each adapter takes only the slice it needs (e.g.
//! `Crypto::from_settings(&settings.crypto)`) and the wiring in `main` reads by
//! concern. `from_env` still aggregates *all* configuration problems — missing
//! required vars and malformed numerics — into a single error.

use std::sync::OnceLock;

static DEV_MODE: OnceLock<bool> = OnceLock::new();

/// Returns true if dev mode is enabled (detailed error messages).
pub fn is_dev_mode() -> bool {
    *DEV_MODE.get().unwrap_or(&false)
}

fn env_is_true(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v.eq_ignore_ascii_case("true"))
}

/// Read an optional env var, treating a *present but empty* value the same as
/// unset. `.env` files commonly ship empty placeholders (e.g. `SMTP_HOST=`);
/// without this, `var().ok()` yields `Some("")` and feature-enable checks that
/// only test for `Some(_)` would wrongly activate with a blank value.
fn optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Read a required env var that has already passed the presence check in
/// [`collect_missing`]. The `.expect("validated")` is unreachable on the live
/// path: `from_env` returns the aggregated error before any of these run.
fn required_env(name: &str) -> String {
    std::env::var(name).expect("validated")
}

fn require_env(problems: &mut Vec<String>, name: &str) {
    if optional_env(name).is_none() {
        problems.push(format!("missing required environment variable: {name}"));
    }
}

/// Derive a CORS `Origin` (scheme + authority, no path or trailing slash) from
/// a base URL such as `https://app.example.com/`.
///
/// An HTTP `Origin` header is only ever scheme + host + optional port; a value
/// carrying a path or trailing slash never matches the browser's `Origin`,
/// silently breaking credentialed CORS. We normalize here so a forgiving
/// `FRONTEND_BASE_URL` config can't break CORS or panic the router at boot.
///
/// # Errors
///
/// Returns an error string if the URL has no `http`/`https` scheme, is missing
/// an authority, or would not be a valid HTTP header value.
fn derive_origin(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim();
    let (scheme, rest) = trimmed
        .split_once("://")
        .ok_or_else(|| format!("FRONTEND_BASE_URL must include a scheme (got {trimmed:?})"))?;

    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(format!(
            "FRONTEND_BASE_URL scheme must be http or https (got {scheme:?})"
        ));
    }

    // Authority is everything up to the first '/', '?' or '#'.
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('.');
    if authority.is_empty() {
        return Err(format!(
            "FRONTEND_BASE_URL must include a host (got {trimmed:?})"
        ));
    }

    let origin = format!("{}://{authority}", scheme.to_ascii_lowercase());
    if origin.parse::<axum::http::HeaderValue>().is_err() {
        return Err(format!(
            "FRONTEND_BASE_URL does not yield a valid origin (got {trimmed:?})"
        ));
    }
    Ok(origin)
}

/// Parse an optional numeric env var, falling back to `default` when unset.
///
/// A *present but unparseable* value is recorded in `problems` (rather than
/// panicking) so every configuration error surfaces together in one aggregated
/// message. Returns `default` on the error path so loading can continue
/// and collect any further problems.
fn parse_env<T>(name: &str, default: T, problems: &mut Vec<String>) -> T
where
    T: std::str::FromStr,
{
    match std::env::var(name) {
        Ok(raw) => raw.parse::<T>().unwrap_or_else(|_| {
            problems.push(format!("{name} must be a valid number (got {raw:?})"));
            default
        }),
        Err(_) => default,
    }
}

/// Collect the names of every required-but-missing environment variable, given
/// the enable flags that gate the optional feature groups.
fn collect_missing(problems: &mut Vec<String>) {
    for var in [
        "DATABASE_URL",
        "SERVER_HOST",
        "SERVER_PORT",
        "DEV_MODE",
        "APP_NAME",
        "FRONTEND_BASE_URL",
        "ARGON2_MEMORY_COST",
        "ARGON2_TIME_COST",
        "ARGON2_PARALLELISM",
        "JWT_EXPIRATION_MINUTES",
        "REFRESH_TOKEN_EXPIRATION_DAYS",
        "JWT_PRIVATE_KEY",
        "JWT_PUBLIC_KEY",
        "CSRF_SECRET",
        "SECRETS_ENCRYPTION_KEY",
    ] {
        require_env(problems, var);
    }

    if env_is_true("GOOGLE_OAUTH_ENABLED") {
        for var in [
            "GOOGLE_CLIENT_ID",
            "GOOGLE_CLIENT_SECRET",
            "GOOGLE_REDIRECT_URI",
        ] {
            require_env(problems, var);
        }
    }
}

/// Postgres connection settings.
#[derive(Debug, Clone)]
pub struct DatabaseSettings {
    pub url: String,
    pub max_connections: u32,
}

/// HTTP server + top-level app identity.
#[derive(Debug, Clone)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
    pub dev_mode: bool,
    pub app_name: String,
    pub frontend_base_url: String,
    /// The CORS `Origin` (scheme + host + optional port, no path/trailing
    /// slash) derived and validated from `frontend_base_url` at load time, so
    /// the router never has to parse an untrusted value while building CORS.
    pub frontend_origin: String,
}

impl ServerSettings {
    /// Returns the full server address as `host:port`.
    #[must_use]
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Google OAuth credentials (only populated when `enabled`).
#[derive(Debug, Clone)]
pub struct GoogleOAuthSettings {
    pub enabled: bool,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub redirect_uri: Option<String>,
}

/// SMTP transport settings (optional in dev, required in prod).
#[derive(Debug, Clone)]
pub struct SmtpSettings {
    pub host: Option<String>,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_address: Option<String>,
    pub from_name: Option<String>,
}

/// Bot-protection settings (Turnstile + MX validation).
#[derive(Debug, Clone)]
pub struct BotProtectionSettings {
    pub turnstile_secret_key: Option<String>,
    pub mx_validation_enabled: bool,
}

/// Crypto / JWT material. Consumed wholesale by [`Crypto::from_settings`].
///
/// [`Crypto::from_settings`]: crate::shared::crypto::Crypto::from_settings
#[derive(Debug, Clone)]
pub struct CryptoSettings {
    pub argon2_memory_cost: u32,
    pub argon2_time_cost: u32,
    pub argon2_parallelism: u32,
    pub jwt_private_key: String,
    pub jwt_public_key: String,
    pub jwt_expiration_minutes: i64,
    pub refresh_token_expiration_days: i64,
    pub csrf_secret_b64: String,
    /// Base64-encoded 32-byte key used to encrypt operational secrets at rest
    /// (third-party tokens the app must store) via AES-256-GCM.
    pub secrets_encryption_key_b64: String,
}

/// Application configuration settings, grouped by concern.
#[derive(Debug, Clone)]
pub struct Settings {
    pub server: ServerSettings,
    pub database: DatabaseSettings,
    pub crypto: CryptoSettings,
    pub google_oauth: GoogleOAuthSettings,
    pub smtp: SmtpSettings,
    pub bot_protection: BotProtectionSettings,
}

impl Settings {
    /// Loads settings from environment variables.
    ///
    /// # Errors
    ///
    /// Returns a [`config::ConfigError`] listing **all** configuration problems
    /// at once — every missing required variable and every present-but-malformed
    /// numeric variable — instead of panicking on the first bad value.
    ///
    /// # Panics
    ///
    /// Does not panic on user misconfiguration: all such problems are returned as
    /// the aggregated error above. The internal `.expect("validated")` calls are
    /// unreachable because they run only after the presence check has confirmed
    /// every required variable is set.
    #[allow(clippy::too_many_lines)]
    pub fn from_env() -> Result<Self, config::ConfigError> {
        dotenvy::dotenv().ok();

        let mut problems: Vec<String> = Vec::new();
        collect_missing(&mut problems);

        // Numeric vars are parsed leniently into `problems` so a malformed value
        // (e.g. `SERVER_PORT=foo`) joins the aggregated error rather than panicking.
        let database_max_connections = parse_env("DATABASE_MAX_CONNECTIONS", 10, &mut problems);
        let server_port = parse_env("SERVER_PORT", 0u16, &mut problems);
        let smtp_port = parse_env("SMTP_PORT", 587, &mut problems);
        let argon2_memory_cost = parse_env("ARGON2_MEMORY_COST", 0, &mut problems);
        let argon2_time_cost = parse_env("ARGON2_TIME_COST", 0, &mut problems);
        let argon2_parallelism = parse_env("ARGON2_PARALLELISM", 0, &mut problems);
        let jwt_expiration_minutes = parse_env("JWT_EXPIRATION_MINUTES", 0, &mut problems);
        let refresh_token_expiration_days =
            parse_env("REFRESH_TOKEN_EXPIRATION_DAYS", 0, &mut problems);

        // Derive and validate the CORS origin now so a malformed
        // FRONTEND_BASE_URL joins the aggregated error rather than panicking
        // while the router builds its CORS layer. A `None` here means the var
        // is absent, which `collect_missing` has already recorded.
        let frontend_origin = optional_env("FRONTEND_BASE_URL")
            .and_then(|base| derive_origin(&base).map_err(|e| problems.push(e)).ok());

        let dev_mode = env_is_true("DEV_MODE");

        if !problems.is_empty() {
            return Err(config::ConfigError::Message(format!(
                "\n\nConfiguration errors:\n  - {}\n\nCheck your .env file.\n",
                problems.join("\n  - ")
            )));
        }

        // Past this point every required var is present; `required_env` only runs
        // once the presence check above has confirmed the value exists.
        let _ = DEV_MODE.set(dev_mode);

        let server = ServerSettings {
            host: required_env("SERVER_HOST"),
            port: server_port,
            dev_mode,
            app_name: required_env("APP_NAME"),
            frontend_base_url: required_env("FRONTEND_BASE_URL"),
            frontend_origin: frontend_origin.expect("validated"),
        };

        let database = DatabaseSettings {
            url: required_env("DATABASE_URL"),
            max_connections: database_max_connections,
        };

        let crypto = CryptoSettings {
            argon2_memory_cost,
            argon2_time_cost,
            argon2_parallelism,
            jwt_private_key: required_env("JWT_PRIVATE_KEY"),
            jwt_public_key: required_env("JWT_PUBLIC_KEY"),
            jwt_expiration_minutes,
            refresh_token_expiration_days,
            csrf_secret_b64: required_env("CSRF_SECRET"),
            secrets_encryption_key_b64: required_env("SECRETS_ENCRYPTION_KEY"),
        };

        let google_oauth = GoogleOAuthSettings {
            enabled: env_is_true("GOOGLE_OAUTH_ENABLED"),
            client_id: optional_env("GOOGLE_CLIENT_ID"),
            client_secret: optional_env("GOOGLE_CLIENT_SECRET"),
            redirect_uri: optional_env("GOOGLE_REDIRECT_URI"),
        };

        let smtp = SmtpSettings {
            host: optional_env("SMTP_HOST"),
            port: smtp_port,
            username: optional_env("SMTP_USERNAME"),
            password: optional_env("SMTP_PASSWORD"),
            from_address: optional_env("SMTP_FROM_ADDRESS"),
            from_name: optional_env("SMTP_FROM_NAME"),
        };

        let bot_protection = BotProtectionSettings {
            turnstile_secret_key: optional_env("TURNSTILE_SECRET_KEY"),
            mx_validation_enabled: env_is_true("MX_VALIDATION_ENABLED"),
        };

        Ok(Self {
            server,
            database,
            crypto,
            google_oauth,
            smtp,
            bot_protection,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_dev_mode_does_not_panic() {
        let _ = is_dev_mode();
    }

    #[test]
    fn server_address_formats_correctly() {
        let server = ServerSettings {
            host: "0.0.0.0".to_string(),
            port: 4322,
            dev_mode: false,
            app_name: "Test".to_string(),
            frontend_base_url: "http://localhost:4321".to_string(),
            frontend_origin: "http://localhost:4321".to_string(),
        };

        assert_eq!(server.address(), "0.0.0.0:4322");
    }

    #[test]
    fn derive_origin_strips_path_and_trailing_slash() {
        assert_eq!(
            derive_origin("https://app.example.com/").unwrap(),
            "https://app.example.com"
        );
        assert_eq!(
            derive_origin("https://app.example.com/path?q=1#frag").unwrap(),
            "https://app.example.com"
        );
        assert_eq!(
            derive_origin("http://localhost:4321").unwrap(),
            "http://localhost:4321"
        );
        // Scheme is lowercased; surrounding whitespace trimmed.
        assert_eq!(
            derive_origin("  HTTPS://App.Example.com/  ").unwrap(),
            "https://App.Example.com"
        );
    }

    #[test]
    fn derive_origin_rejects_malformed_values() {
        assert!(derive_origin("app.example.com").is_err()); // no scheme
        assert!(derive_origin("ftp://example.com").is_err()); // bad scheme
        assert!(derive_origin("https://").is_err()); // no host
    }

    #[test]
    fn derive_origin_preserves_explicit_port() {
        assert_eq!(
            derive_origin("http://example.com:8443/foo").unwrap(),
            "http://example.com:8443"
        );
    }

    #[test]
    fn derive_origin_rejects_authority_made_only_of_dots() {
        // Trailing dots are stripped; an all-dot authority collapses to empty.
        assert!(derive_origin("https://.../path").is_err());
    }

    // ---- pure helpers that read the environment -----------------------------
    //
    // Env vars are process-global, so every test that sets or removes one is
    // serialized through `ENV_GUARD`. Each test restores the vars it touched
    // before releasing the lock so state never leaks to another test.

    use std::sync::Mutex;

    static ENV_GUARD: Mutex<()> = Mutex::new(());

    /// Set a batch of env vars, run `body`, then restore each var to its prior
    /// value (or remove it if it was unset). The `ENV_GUARD` lock is held for the
    /// whole duration so concurrent tests never observe partial state.
    fn with_env_vars<F: FnOnce()>(vars: &[(&str, &str)], body: F) {
        let _lock = ENV_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, v)| {
                let prior = std::env::var(k).ok();
                // SAFETY: tests are serialized via ENV_GUARD; no other thread
                // reads the environment concurrently.
                unsafe { std::env::set_var(k, v) };
                ((*k).to_string(), prior)
            })
            .collect();

        body();

        for (k, prior) in saved {
            unsafe {
                match prior {
                    Some(v) => std::env::set_var(&k, v),
                    None => std::env::remove_var(&k),
                }
            }
        }
    }

    /// Run `body` with the named env var guaranteed-absent, then restore it.
    fn with_env_removed<F: FnOnce()>(name: &str, body: F) {
        let _lock = ENV_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior = std::env::var(name).ok();
        unsafe { std::env::remove_var(name) };

        body();

        unsafe {
            if let Some(v) = prior {
                std::env::set_var(name, v);
            }
        }
    }

    #[test]
    fn env_is_true_matches_case_insensitively() {
        with_env_vars(&[("TEST_FLAG_BOOL", "TrUe")], || {
            assert!(env_is_true("TEST_FLAG_BOOL"));
        });
        with_env_vars(&[("TEST_FLAG_BOOL", "false")], || {
            assert!(!env_is_true("TEST_FLAG_BOOL"));
        });
        with_env_vars(&[("TEST_FLAG_BOOL", "1")], || {
            assert!(!env_is_true("TEST_FLAG_BOOL"));
        });
        with_env_removed("TEST_FLAG_BOOL_UNSET", || {
            assert!(!env_is_true("TEST_FLAG_BOOL_UNSET"));
        });
    }

    #[test]
    fn optional_env_treats_empty_as_unset() {
        with_env_vars(&[("TEST_OPT_VAL", "value")], || {
            assert_eq!(optional_env("TEST_OPT_VAL"), Some("value".to_string()));
        });
        with_env_vars(&[("TEST_OPT_VAL", "")], || {
            assert_eq!(optional_env("TEST_OPT_VAL"), None);
        });
        with_env_removed("TEST_OPT_UNSET", || {
            assert_eq!(optional_env("TEST_OPT_UNSET"), None);
        });
    }

    #[test]
    fn require_env_records_missing_and_empty() {
        with_env_removed("TEST_REQ_MISSING", || {
            let mut problems = Vec::new();
            require_env(&mut problems, "TEST_REQ_MISSING");
            assert_eq!(problems.len(), 1);
            assert!(problems[0].contains("TEST_REQ_MISSING"));
        });

        with_env_vars(&[("TEST_REQ_EMPTY", "")], || {
            let mut problems = Vec::new();
            require_env(&mut problems, "TEST_REQ_EMPTY");
            assert_eq!(problems.len(), 1);
        });

        with_env_vars(&[("TEST_REQ_SET", "x")], || {
            let mut problems = Vec::new();
            require_env(&mut problems, "TEST_REQ_SET");
            assert_eq!(problems, Vec::<String>::new());
        });
    }

    #[test]
    fn parse_env_returns_default_when_unset() {
        with_env_removed("TEST_PARSE_UNSET", || {
            let mut problems = Vec::new();
            let v: u16 = parse_env("TEST_PARSE_UNSET", 42, &mut problems);
            assert_eq!(v, 42);
            assert_eq!(problems, Vec::<String>::new());
        });
    }

    #[test]
    fn parse_env_parses_valid_value() {
        with_env_vars(&[("TEST_PARSE_OK", "8080")], || {
            let mut problems = Vec::new();
            let v: u16 = parse_env("TEST_PARSE_OK", 0, &mut problems);
            assert_eq!(v, 8080);
            assert_eq!(problems, Vec::<String>::new());
        });
    }

    #[test]
    fn parse_env_records_problem_on_malformed_value() {
        with_env_vars(&[("TEST_PARSE_BAD", "not-a-number")], || {
            let mut problems = Vec::new();
            let v: u16 = parse_env("TEST_PARSE_BAD", 7, &mut problems);
            // Falls back to default so loading can continue and collect more.
            assert_eq!(v, 7);
            assert_eq!(problems.len(), 1);
            assert!(problems[0].contains("TEST_PARSE_BAD"));
            assert!(problems[0].contains("valid number"));
        });
    }

    #[test]
    fn collect_missing_reports_each_required_var_when_all_absent() {
        // Remove every base + feature-flag var so collect_missing sees a clean
        // slate, then assert the core required set is reported.
        let all = [
            "DATABASE_URL",
            "SERVER_HOST",
            "SERVER_PORT",
            "DEV_MODE",
            "APP_NAME",
            "FRONTEND_BASE_URL",
            "ARGON2_MEMORY_COST",
            "ARGON2_TIME_COST",
            "ARGON2_PARALLELISM",
            "JWT_EXPIRATION_MINUTES",
            "REFRESH_TOKEN_EXPIRATION_DAYS",
            "JWT_PRIVATE_KEY",
            "JWT_PUBLIC_KEY",
            "CSRF_SECRET",
            "SECRETS_ENCRYPTION_KEY",
            "GOOGLE_OAUTH_ENABLED",
        ];
        let _lock = ENV_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved: Vec<(&str, Option<String>)> =
            all.iter().map(|k| (*k, std::env::var(k).ok())).collect();
        unsafe {
            for k in all {
                std::env::remove_var(k);
            }
        }

        let mut problems = Vec::new();
        collect_missing(&mut problems);

        assert_eq!(problems.len(), 15);
        assert!(problems.iter().any(|p| p.contains("DATABASE_URL")));
        assert!(
            problems
                .iter()
                .any(|p| p.contains("SECRETS_ENCRYPTION_KEY"))
        );
        assert!(!problems.iter().any(|p| p.contains("GOOGLE_CLIENT_ID")));

        unsafe {
            for (k, prior) in saved {
                match prior {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn from_env_aggregates_malformed_numeric_var() {
        // Drive `from_env` with every required var present and valid except a
        // present-but-malformed numeric var. This exercises the aggregated
        // error-return path deterministically: because every required var is
        // set here, the result does not depend on whether a `.env` file exists
        // in the working directory (`from_env` calls `dotenvy::dotenv()`, which
        // never overrides an already-set var). The missing-required-var path is
        // covered separately by `collect_missing_*`.
        let required_valid = [
            ("DATABASE_URL", "postgres://u:p@localhost/db"),
            ("SERVER_HOST", "0.0.0.0"),
            ("DEV_MODE", "true"),
            ("APP_NAME", "Test"),
            ("FRONTEND_BASE_URL", "http://localhost:4321"),
            ("ARGON2_MEMORY_COST", "19456"),
            ("ARGON2_TIME_COST", "2"),
            ("ARGON2_PARALLELISM", "1"),
            ("JWT_EXPIRATION_MINUTES", "15"),
            ("REFRESH_TOKEN_EXPIRATION_DAYS", "30"),
            ("JWT_PRIVATE_KEY", "private"),
            ("JWT_PUBLIC_KEY", "public"),
            ("CSRF_SECRET", "csrf-secret"),
            ("SECRETS_ENCRYPTION_KEY", "enc-key"),
        ];
        // Feature flags off so their conditionally-required vars aren't demanded.
        let flags = ["GOOGLE_OAUTH_ENABLED"];

        let _lock = ENV_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved: Vec<(&str, Option<String>)> = required_valid
            .iter()
            .map(|(k, _)| *k)
            .chain(flags)
            .chain(std::iter::once("SERVER_PORT"))
            .map(|k| (k, std::env::var(k).ok()))
            .collect();
        unsafe {
            for (k, v) in required_valid {
                std::env::set_var(k, v);
            }
            for k in flags {
                std::env::set_var(k, "false");
            }
            // Present-but-malformed numeric var joins the aggregated error.
            std::env::set_var("SERVER_PORT", "not-a-port");
        }

        let result = Settings::from_env();

        unsafe {
            for (k, prior) in saved {
                match prior {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }

        let err = result.expect_err("malformed numeric var must produce an error");
        let msg = err.to_string();
        assert!(msg.contains("Configuration errors"));
        assert!(msg.contains("SERVER_PORT must be a valid number"));
    }
}
