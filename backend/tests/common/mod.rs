use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::body::Body;
use axum::http::header;
use overkill_todo::domain::entities::OAuthProvider;
use overkill_todo::domain::errors::DomainError;
use overkill_todo::{
    application::{
        ports::{
            EmailService, MxValidator, OAuthProviderClient, TurnstileService,
            outgoing::ClientReportSink,
        },
        use_cases::{
            CancelAccountDeletionUseCase, ChangePasswordUseCase, CompleteSignupUseCase,
            ExportUserDataUseCase, GetCurrentUserUseCase, ListAuditEventsUseCase, ListUsersUseCase,
            LoginUserUseCase, OAuthCallbackUseCase, OAuthUnlinkUseCase, RefreshTokenUseCase,
            RegisterUserUseCase, RequestAccountDeletionUseCase, RequestPasswordResetUseCase,
            ResendVerificationUseCase, ResetPasswordUseCase, RevokeTokenUseCase,
            SendVerificationCodeUseCase, SetPasswordUseCase, SetUserRoleUseCase,
            ValidateResetCodeUseCase, VerifyEmailUseCase, VerifyMagicLinkUseCase,
            telemetry::{
                RecordClientTelemetryUseCase, record_client_telemetry::DEFAULT_MAX_EVENTS_PER_SEC,
            },
        },
    },
    infrastructure::{
        api::{
            RouterDeps, create_router,
            handlers::{
                admin_handler::AdminHandlerDeps, auth_handler::AuthHandlerDeps,
                client_telemetry_handler::ClientTelemetryHandlerDeps,
                oauth_handler::OAuthHandlerDeps, profile_handler::ProfileHandlerDeps,
                verification_handler::VerificationHandlerDeps,
            },
        },
        persistence::{
            Persistence,
            postgres::{self, Database},
        },
        services::{
            Argon2Hasher, ConsoleEmailService, CryptoJwtService, CryptoTokenService,
            NoOpMxValidator, NoOpTurnstileService, RandomCodeGenerator,
        },
    },
};
use sqlx::PgPool;

use overkill_todo::infrastructure::crypto::Crypto;
use overkill_todo::infrastructure::telemetry::client_reports::TelemetryClientReportSink;
use overkill_todo::infrastructure::telemetry::metrics::AuthMetrics;

static TEST_CRYPTO: OnceLock<Arc<Crypto>> = OnceLock::new();

/// A `<kid>:<base64 secret>` token key with a fixed 64-byte test secret.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; not all of them use this"
)]
pub fn test_token_key(kid: &str, fill: u8) -> String {
    use base64::Engine;
    format!(
        "{kid}:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([fill; 64])
    )
}

/// Build a crypto handle with test parameters and the given token keys
/// (`JWT_SIGNING_KEY` and `JWT_TRUSTED_KEYS` values).
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; not all of them use this"
)]
pub fn crypto_with_token_keys(signing: &str, trusted: Option<&str>) -> Arc<Crypto> {
    // `Crypto::for_testing()` is library-test-only (`cfg(test)`) and not
    // reachable from this integration-test crate, so build the slice here.
    let crypto = overkill_todo::infrastructure::config::CryptoSettings {
        argon2_memory_cost: 4096,
        argon2_time_cost: 1,
        argon2_parallelism: 1,
        jwt_signing_key: signing.to_string(),
        jwt_trusted_keys: trusted.map(str::to_string),
        jwt_expiration_minutes: 15,
        refresh_token_expiration_days: 30,
        csrf_secret_b64: "MWq42LqA6RbYEsgrc0w2Ik9YydReTlRwh_D5Ynus2QA".to_string(),
        secrets_encryption_key_b64: "MWq42LqA6RbYEsgrc0w2Ik9YydReTlRwh_D5Ynus2QA".to_string(),
    };
    Arc::new(Crypto::from_settings(&crypto).expect("test crypto settings are valid"))
}

fn test_crypto() -> Arc<Crypto> {
    TEST_CRYPTO
        .get_or_init(|| crypto_with_token_keys(&test_token_key("it-a", 0x11), None))
        .clone()
}

/// Build a crypto handle with test parameters.
///
/// Idempotent (cached). Useful for repository-level integration tests that
/// exercise the at-rest secret cipher without building the full HTTP app.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; not all of them use this"
)]
pub fn ensure_test_crypto() -> Arc<Crypto> {
    test_crypto()
}

/// Extract a cookie value by name from a response's `Set-Cookie` headers.
///
/// The native body-token path was removed, so tests read the session token and
/// CSRF token from the cookies the server sets, exactly like a browser.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; not all of them use this"
)]
pub fn cookie_value(resp: &axum::http::Response<Body>, name: &str) -> Option<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| {
            let kv = c.split(';').next()?;
            let (k, val) = kv.split_once('=')?;
            (k.trim() == name).then(|| val.trim().to_string())
        })
}

/// Mint a session JWT against the test crypto keypair, with explicit control
/// over the `verified` flag, role, and expiry. Used by security tests that need
/// a token the normal signup flow won't produce (e.g. an unverified session, or
/// an already-expired token).
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; not all of them use this"
)]
pub fn mint_session_jwt(
    user_id: uuid::Uuid,
    email_verified: bool,
    role: &str,
    expiration_minutes: i64,
) -> String {
    let crypto = test_crypto();
    overkill_todo::infrastructure::api::create_jwt(
        user_id,
        email_verified,
        role,
        crypto.tokens(),
        expiration_minutes,
    )
    .expect("failed to mint test JWT")
}

/// Generate a user-bound CSRF token using the test crypto's CSRF secret, exactly
/// as the server does at session-issue time. Used to forge a token bound to a
/// *different* user id.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; not all of them use this"
)]
pub fn mint_csrf_token(user_id: &str) -> String {
    let crypto = test_crypto();
    overkill_todo::infrastructure::api::generate_csrf_token(crypto.csrf_secret(), user_id)
}

/// Build a fully wired test application from a sqlx-provided pool.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; not all of them use this"
)]
pub fn create_test_app(pool: PgPool) -> Router {
    create_test_app_full(
        pool,
        test_crypto(),
        console_email(),
        HashMap::new(),
        no_op_turnstile(),
        AuthMetrics::from_global(),
    )
}

/// Build the test app with the given crypto handle, e.g. one holding different
/// token keys to exercise key rotation.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; not all of them use this"
)]
pub fn create_test_app_with_crypto(pool: PgPool, crypto: Arc<Crypto>) -> Router {
    create_test_app_full(
        pool,
        crypto,
        console_email(),
        HashMap::new(),
        no_op_turnstile(),
        AuthMetrics::from_global(),
    )
}

/// Build the test app with the given OAuth provider clients registered, so the
/// real (non-bypass) start → callback flow can run against a fake provider.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; only the OAuth integration binary uses this"
)]
pub fn create_test_app_with_oauth(
    pool: PgPool,
    oauth_clients: HashMap<OAuthProvider, Arc<dyn OAuthProviderClient>>,
) -> Router {
    create_test_app_full(
        pool,
        test_crypto(),
        console_email(),
        oauth_clients,
        no_op_turnstile(),
        AuthMetrics::from_global(),
    )
}

/// Build the test app recording auth outcomes on `auth_metrics`, e.g.
/// instruments on a meter backed by an in-memory exporter.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; only the auth metrics binary uses this"
)]
pub fn create_test_app_with_auth_metrics(pool: PgPool, auth_metrics: AuthMetrics) -> Router {
    create_test_app_full(
        pool,
        test_crypto(),
        console_email(),
        HashMap::new(),
        no_op_turnstile(),
        auth_metrics,
    )
}

/// The email service every builder uses unless a test wants delivery to fail.
fn no_op_turnstile() -> Arc<dyn TurnstileService> {
    Arc::new(NoOpTurnstileService)
}

/// Build the test app with bot protection switched on, using the given
/// Turnstile service in place of the default no-op (which reports disabled).
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; only the Turnstile integration binary uses this"
)]
pub fn create_test_app_with_turnstile(
    pool: PgPool,
    turnstile_service: Arc<dyn TurnstileService>,
) -> Router {
    create_test_app_full(
        pool,
        test_crypto(),
        console_email(),
        HashMap::new(),
        turnstile_service,
        AuthMetrics::from_global(),
    )
}

/// Bot protection on, with auth metrics recorded on the given instruments.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; only the auth metrics binary uses this"
)]
pub fn create_test_app_with_turnstile_and_auth_metrics(
    pool: PgPool,
    turnstile_service: Arc<dyn TurnstileService>,
    auth_metrics: AuthMetrics,
) -> Router {
    create_test_app_full(
        pool,
        test_crypto(),
        console_email(),
        HashMap::new(),
        turnstile_service,
        auth_metrics,
    )
}

fn console_email() -> Arc<dyn EmailService> {
    Arc::new(ConsoleEmailService::new("Test".to_string()))
}

/// An email service whose first `fail_count` sends fail, after which it behaves
/// like [`ConsoleEmailService`].
///
/// Lets a test drive a mail outage and then its recovery in one run — the case
/// that matters is a registration whose email fails, followed by a resend that
/// works, which is only reachable if the failed send did not arm the cooldown.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; not all of them use this"
)]
pub struct FailFirstEmailService {
    sends: AtomicUsize,
    fail_count: usize,
    inner: ConsoleEmailService,
}

#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; not all of them use this"
)]
impl FailFirstEmailService {
    pub fn new(fail_count: usize) -> Self {
        Self {
            sends: AtomicUsize::new(0),
            fail_count,
            inner: ConsoleEmailService::new("Test".to_string()),
        }
    }

    /// True while the next send is still inside the failing window.
    fn should_fail(&self) -> bool {
        self.sends.fetch_add(1, Ordering::SeqCst) < self.fail_count
    }
}

#[async_trait::async_trait]
impl EmailService for FailFirstEmailService {
    async fn send_verification_email(
        &self,
        to: &str,
        code: &str,
        first_name: &str,
        magic_link: Option<String>,
    ) -> Result<(), DomainError> {
        if self.should_fail() {
            return Err(DomainError::ExternalService("relay unreachable".into()));
        }
        self.inner
            .send_verification_email(to, code, first_name, magic_link)
            .await
    }

    async fn send_password_reset_email(
        &self,
        to: &str,
        code: &str,
        first_name: &str,
    ) -> Result<(), DomainError> {
        if self.should_fail() {
            return Err(DomainError::ExternalService("relay unreachable".into()));
        }
        self.inner
            .send_password_reset_email(to, code, first_name)
            .await
    }

    async fn notify_account_exists(&self, to: &str, first_name: &str) -> Result<(), DomainError> {
        if self.should_fail() {
            return Err(DomainError::ExternalService("relay unreachable".into()));
        }
        self.inner.notify_account_exists(to, first_name).await
    }
}

/// Like [`create_test_app`] but with an injectable email service, so tests can
/// drive the paths where delivery fails.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; not all of them use this"
)]
pub fn create_test_app_with_email_service(
    pool: PgPool,
    email_service: Arc<dyn EmailService>,
) -> Router {
    create_test_app_full(
        pool,
        test_crypto(),
        email_service,
        HashMap::new(),
        no_op_turnstile(),
        AuthMetrics::from_global(),
    )
}

/// Build the test app recording browser reports through `sink`, with a
/// recording budget of `max_events_per_sec`.
#[allow(
    dead_code,
    reason = "`common` is compiled into every test binary; only the client telemetry binary uses this"
)]
pub fn create_test_app_with_client_telemetry(
    pool: PgPool,
    sink: Arc<dyn ClientReportSink>,
    max_events_per_sec: u32,
) -> Router {
    create_test_app_inner(
        pool,
        test_crypto(),
        console_email(),
        HashMap::new(),
        no_op_turnstile(),
        AuthMetrics::from_global(),
        client_telemetry(sink, max_events_per_sec),
    )
}

fn client_telemetry(
    sink: Arc<dyn ClientReportSink>,
    max_events_per_sec: u32,
) -> Arc<ClientTelemetryHandlerDeps> {
    Arc::new(ClientTelemetryHandlerDeps {
        record: Arc::new(RecordClientTelemetryUseCase::new(sink, max_events_per_sec)),
    })
}

fn create_test_app_full(
    pool: PgPool,
    crypto: Arc<Crypto>,
    email_service: Arc<dyn EmailService>,
    oauth_clients: HashMap<OAuthProvider, Arc<dyn OAuthProviderClient>>,
    turnstile_service: Arc<dyn TurnstileService>,
    auth_metrics: AuthMetrics,
) -> Router {
    create_test_app_inner(
        pool,
        crypto,
        email_service,
        oauth_clients,
        turnstile_service,
        auth_metrics,
        client_telemetry(
            Arc::new(TelemetryClientReportSink::from_global()),
            DEFAULT_MAX_EVENTS_PER_SEC,
        ),
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "wires the whole app the way main.rs does, in the same order"
)]
fn create_test_app_inner(
    pool: PgPool,
    crypto: Arc<Crypto>,
    email_service: Arc<dyn EmailService>,
    oauth_clients: HashMap<OAuthProvider, Arc<dyn OAuthProviderClient>>,
    turnstile_service: Arc<dyn TurnstileService>,
    auth_metrics: AuthMetrics,
    client_telemetry: Arc<ClientTelemetryHandlerDeps>,
) -> Router {
    let code_generator = Arc::new(RandomCodeGenerator::development());
    let jwt_service = Arc::new(CryptoJwtService::new(crypto.clone()));
    let password_hasher = Arc::new(Argon2Hasher::new(crypto.clone()));
    let token_service = Arc::new(CryptoTokenService::new(crypto.clone()));
    let mx_validator: Arc<dyn MxValidator> = Arc::new(NoOpMxValidator);

    let Persistence {
        users: user_repo,
        account_lifecycle: _,
        audit_log: audit_log_repo,
        oauth_accounts: oauth_account_repo,
        refresh_tokens: refresh_token_repo,
        email_verifications: email_verification_repo,
        password_resets: password_reset_repo,
        db,
    } = postgres::wire(Arc::new(Database::from_pool(pool)));

    let register_uc = Arc::new(RegisterUserUseCase::new(
        user_repo.clone(),
        oauth_account_repo.clone(),
        email_service.clone(),
        mx_validator,
    ));
    let login_uc = Arc::new(LoginUserUseCase::new(
        user_repo.clone(),
        password_hasher.clone(),
    ));
    let get_current_user_uc = Arc::new(GetCurrentUserUseCase::new(
        user_repo.clone(),
        oauth_account_repo.clone(),
    ));
    let refresh_token_uc = Arc::new(RefreshTokenUseCase::new(
        refresh_token_repo.clone(),
        user_repo.clone(),
        token_service.clone(),
    ));
    let revoke_token_uc = Arc::new(RevokeTokenUseCase::new(
        refresh_token_repo.clone(),
        token_service.clone(),
    ));
    let send_verification_uc = Arc::new(SendVerificationCodeUseCase::new(
        email_verification_repo.clone(),
        email_service.clone(),
        code_generator.clone(),
        password_hasher.clone(),
        "http://localhost:4321".to_string(),
    ));
    let verify_email_uc = Arc::new(VerifyEmailUseCase::new(
        user_repo.clone(),
        email_verification_repo.clone(),
        password_hasher.clone(),
    ));
    let verify_magic_link_uc = Arc::new(VerifyMagicLinkUseCase::new(
        user_repo.clone(),
        email_verification_repo.clone(),
    ));
    let resend_verification_uc = Arc::new(ResendVerificationUseCase::new(
        user_repo.clone(),
        email_verification_repo,
        email_service.clone(),
        code_generator.clone(),
        password_hasher.clone(),
    ));
    let complete_signup_uc = Arc::new(CompleteSignupUseCase::new(
        user_repo.clone(),
        jwt_service.clone(),
        password_hasher.clone(),
    ));
    let request_password_reset_uc = Arc::new(RequestPasswordResetUseCase::new(
        user_repo.clone(),
        password_reset_repo.clone(),
        email_service,
        code_generator,
        password_hasher.clone(),
    ));
    let validate_reset_code_uc = Arc::new(ValidateResetCodeUseCase::new(
        user_repo.clone(),
        password_reset_repo.clone(),
        password_hasher.clone(),
        jwt_service.clone(),
    ));
    let reset_password_uc = Arc::new(ResetPasswordUseCase::new(
        user_repo.clone(),
        password_reset_repo,
        refresh_token_repo.clone(),
        jwt_service,
        password_hasher.clone(),
    ));
    let oauth_callback_uc = Arc::new(OAuthCallbackUseCase::new(
        user_repo.clone(),
        oauth_account_repo.clone(),
    ));
    let oauth_unlink_uc = Arc::new(OAuthUnlinkUseCase::new(
        user_repo.clone(),
        oauth_account_repo.clone(),
    ));
    let change_password_uc = Arc::new(ChangePasswordUseCase::new(
        user_repo.clone(),
        password_hasher.clone(),
        refresh_token_repo,
        token_service,
    ));
    let export_user_data_uc = Arc::new(ExportUserDataUseCase::new(
        user_repo.clone(),
        oauth_account_repo.clone(),
    ));
    let request_account_deletion_uc = Arc::new(RequestAccountDeletionUseCase::new(
        user_repo.clone(),
        password_hasher.clone(),
    ));
    let cancel_account_deletion_uc = Arc::new(CancelAccountDeletionUseCase::new(user_repo.clone()));
    let set_password_uc = Arc::new(SetPasswordUseCase::new(user_repo.clone(), password_hasher));

    let auth_deps = Arc::new(AuthHandlerDeps {
        register_use_case: register_uc,
        login_use_case: login_uc,
        get_current_user_use_case: get_current_user_uc,
        refresh_token_use_case: refresh_token_uc.clone(),
        revoke_token_use_case: revoke_token_uc,
        send_verification_code_use_case: send_verification_uc,
        turnstile_service: turnstile_service.clone(),
        crypto: crypto.clone(),
        auth_metrics: auth_metrics.clone(),
    });

    let verification_deps = Arc::new(VerificationHandlerDeps {
        verify_email_use_case: verify_email_uc,
        verify_magic_link_use_case: verify_magic_link_uc,
        resend_verification_use_case: resend_verification_uc,
        complete_signup_use_case: complete_signup_uc,
        request_password_reset_use_case: request_password_reset_uc,
        validate_reset_code_use_case: validate_reset_code_uc,
        reset_password_use_case: reset_password_uc,
        refresh_token_use_case: refresh_token_uc.clone(),
        turnstile_service,
        frontend_base_url: "http://localhost:4321".to_string(),
        crypto: crypto.clone(),
        auth_metrics,
    });

    let oauth_deps = Arc::new(OAuthHandlerDeps {
        callback_use_case: oauth_callback_uc,
        unlink_use_case: oauth_unlink_uc,
        refresh_token_use_case: refresh_token_uc,
        oauth_clients,
        crypto: crypto.clone(),
    });

    let profile_deps = Arc::new(ProfileHandlerDeps {
        change_password_use_case: change_password_uc,
        set_password_use_case: set_password_uc,
        export_user_data_use_case: export_user_data_uc,
        request_account_deletion_use_case: request_account_deletion_uc,
        cancel_account_deletion_use_case: cancel_account_deletion_uc,
    });

    let admin_deps = Arc::new(AdminHandlerDeps {
        list_users_use_case: Arc::new(ListUsersUseCase::new(user_repo.clone())),
        set_user_role_use_case: Arc::new(SetUserRoleUseCase::new(user_repo.clone())),
        list_audit_events_use_case: Arc::new(ListAuditEventsUseCase::new(audit_log_repo)),
    });

    create_router(
        RouterDeps {
            auth: auth_deps,
            verification: verification_deps,
            oauth: oauth_deps,
            profile: profile_deps,
            admin: admin_deps,
            client_telemetry,
            db,
            crypto,
            users: user_repo,
        },
        "http://localhost:4321",
    )
}
