use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use app::domain::errors::DomainError;
use app::{
    application::{
        ports::{EmailService, MxValidator, OAuthAccountRepository, TurnstileService},
        use_cases::{
            CancelAccountDeletionUseCase, ChangePasswordUseCase, CompleteSignupUseCase,
            ExportUserDataUseCase, GetCurrentUserUseCase, ListUsersUseCase, LoginUserUseCase,
            OAuthCallbackUseCase, OAuthUnlinkUseCase, RefreshTokenUseCase, RegisterUserUseCase,
            RequestAccountDeletionUseCase, RequestPasswordResetUseCase, ResendVerificationUseCase,
            ResetPasswordUseCase, RevokeTokenUseCase, SendVerificationCodeUseCase,
            SetPasswordUseCase, SetUserRoleUseCase, ValidateResetCodeUseCase, VerifyEmailUseCase,
            VerifyMagicLinkUseCase,
        },
    },
    infrastructure::{
        api::{
            RouterDeps, create_router,
            handlers::{
                admin_handler::AdminHandlerDeps, auth_handler::AuthHandlerDeps,
                oauth_handler::OAuthHandlerDeps, profile_handler::ProfileHandlerDeps,
                verification_handler::VerificationHandlerDeps,
            },
        },
        persistence::{
            Database, PostgresEmailVerificationRepository, PostgresOAuthAccountRepository,
            PostgresPasswordResetRepository, PostgresRefreshTokenRepository,
            PostgresUserRepository,
        },
        services::{
            Argon2Hasher, ConsoleEmailService, CryptoJwtService, CryptoTokenService,
            NoOpMxValidator, NoOpTurnstileService, RandomCodeGenerator,
        },
    },
};
use axum::Router;
use axum::body::Body;
use axum::http::header;
use sqlx::PgPool;

use app::shared::crypto::Crypto;

static TEST_CRYPTO: OnceLock<Arc<Crypto>> = OnceLock::new();

const TEST_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCrrWYCApN+866p\nxXEZHqkl9DTa/nQKcx1LPwryXBqAEfZen+gf038DjbmmzqggEbYTx62aUTiSAEs0\nNoouFy/+B9UUepaOMkQN/JqtxwdQjGSFb9++WCM6+5X7iXoyzjRg0VFqyA6LGGs7\nx1uxStvifhOcmFxLBXPD5shVYlT7KoERRv1FGsX4vbfuAjAku4MDuq7FDmc2a86P\nDGEcmrqGiEKlHrJgEPdYa8/OHxE6/Qy/UgQUjN0sQS7OjwbP5ef+93s8JE03JbLo\nljuerCLR5JVJmgZ4IQOOh5bdCR+/K3ZbyETOG+5Sop3iHMLwGCBls9BKK0eBYaPd\n9pGqzifzAgMBAAECggEADWV4gkulKtDlHkZT7vvfgyCtRzF36KDW9lozBnp9buT3\n7qZXvLQKQ0K56/RQVkBLjvuT5mjUG0B+ndJrsfEJQ69kxzZRrULq4JSHlWvkfDqY\nVXlHYlkKVr3vFDPEH1bL64op6kU2LVB4fa+uzgTjZ6ci7KNPAGOqhuzpURBdRUQI\n5UGDNcZPl5jWBRb9Jtjjkaxtvm2qSXC7jeV6rvcjkQhWHJFL2xrhT1IRKj8bUrLD\n21aLTM+mLW2iCuDSfOC3pWfZoQEhNkdVqsKFnKVOPIGRDysPqeRfloE9uMT7YK95\nT/aRksVvPE+bEDbKvdTsuOvvTKY2HRu2St+KbKtXkQKBgQDqpIArr6eftuInb2a8\ndYIuIWfI6UV9ernc9uXglo9JNGgXqcMI7wzaP35EkqAFROiXg29jjkUgQw8BDqwt\n437ic+Wf5+EYq8hQ8L0ig+zstt3FQ3v2i/teBZqOUsoCElXeYxR4DNbQCEbYAEs4\nzGRoqrBCQ2wwEbp0wxNswTKGvwKBgQC7TbhHQeA1es2RMvfZHIzO/8IoSyUV1F9/\nVDgX+pcUZ0FZfb2OMIDwb1w/1A+84NnC63iSI9E/6Rr6XSCNJk9eJBMBoZ2AD/4s\ngDLI2iAUrQHtuNIhshPAbCaW6AVw46fwtCDALFhXK3rw12nezrgYWfehODqZH0R/\nke9kxuD/zQKBgCY3WLDE0TZQIRFvMXpsYSGGS1MLu0eun2r/w30KyJeRs4p3Gc7w\nsFUzlisw/mMdUAytWY0LdPLGOUFg1GY88UOMwM+EgsLz55JXQ1bFyebb+c7RKPWE\n4xHzM7MOhNxFhDrORF0xKiOD1ZYEpju//nKVqlOAi7Rgjs/JnfZOmR4dAoGAXcX0\na6B8iunXf0lgMs+LY5tCbn/oqjwwL4RPrRmlh+6GFjLQsx5vf3CRY9dcpbiA7YiM\nTcYm3fNrsUrsoB49ACnZZiiuvM3E+CyawFoMU62mWIj+kyYfo2fCDi1NhA5GJ0lX\nWD8bX1ECdQ+0Dnu6SV7JjmqjU8QGvEOo8fBr90UCgYEAxvr4swdT46qEm8kWHU3I\ng7yR2rbNYP9TUqg2YKTX3B1Ab9uX1S9c9gg2RdPp5Mxs4XSqVfqJr+yfZP3lSyUa\nT6l2cr36CBT9QNCpHNsfnv4ar+RDqxxojd1+5RFJU4Kg9cWbwB/oIB0g5T1q0HqJ\npqUx9x2LMNqJelwDx/57zt4=\n-----END PRIVATE KEY-----";

const TEST_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAq61mAgKTfvOuqcVxGR6p\nJfQ02v50CnMdSz8K8lwagBH2Xp/oH9N/A425ps6oIBG2E8etmlE4kgBLNDaKLhcv\n/gfVFHqWjjJEDfyarccHUIxkhW/fvlgjOvuV+4l6Ms40YNFRasgOixhrO8dbsUrb\n4n4TnJhcSwVzw+bIVWJU+yqBEUb9RRrF+L237gIwJLuDA7quxQ5nNmvOjwxhHJq6\nhohCpR6yYBD3WGvPzh8ROv0Mv1IEFIzdLEEuzo8Gz+Xn/vd7PCRNNyWy6JY7nqwi\n0eSVSZoGeCEDjoeW3Qkfvyt2W8hEzhvuUqKd4hzC8BggZbPQSitHgWGj3faRqs4n\n8wIDAQAB\n-----END PUBLIC KEY-----";

fn test_crypto() -> Arc<Crypto> {
    TEST_CRYPTO
        .get_or_init(|| {
            // Only the crypto slice is needed to build a `Crypto` handle;
            // `Crypto::for_testing()` is library-test-only (`cfg(test)`) and not
            // reachable from this integration-test crate, so build the slice here.
            let crypto = app::infrastructure::config::CryptoSettings {
                argon2_memory_cost: 4096,
                argon2_time_cost: 1,
                argon2_parallelism: 1,
                jwt_private_key: TEST_PRIVATE_KEY.to_string(),
                jwt_public_key: TEST_PUBLIC_KEY.to_string(),
                jwt_expiration_minutes: 15,
                refresh_token_expiration_days: 30,
                csrf_secret_b64: "MWq42LqA6RbYEsgrc0w2Ik9YydReTlRwh_D5Ynus2QA".to_string(),
                secrets_encryption_key_b64: "MWq42LqA6RbYEsgrc0w2Ik9YydReTlRwh_D5Ynus2QA"
                    .to_string(),
            };
            Arc::new(Crypto::from_settings(&crypto).expect("test Argon2 params are valid"))
        })
        .clone()
}

/// Build a crypto handle with test parameters.
///
/// Idempotent (cached). Useful for repository-level integration tests that
/// exercise the at-rest secret cipher without building the full HTTP app.
// Not every test binary uses this; `common` is compiled per binary.
#[allow(dead_code)]
pub fn ensure_test_crypto() -> Arc<Crypto> {
    test_crypto()
}

/// Extract a cookie value by name from a response's `Set-Cookie` headers.
///
/// The native body-token path was removed, so tests read the session token and
/// CSRF token from the cookies the server sets, exactly like a browser.
// Not every test binary uses this; `common` is compiled per binary.
#[allow(dead_code)]
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
// Only the security integration binary uses this; `common` is compiled per binary.
#[allow(dead_code)]
pub fn mint_session_jwt(
    user_id: uuid::Uuid,
    email_verified: bool,
    role: &str,
    expiration_minutes: i64,
) -> String {
    let crypto = test_crypto();
    app::infrastructure::api::create_jwt(
        user_id,
        email_verified,
        role,
        crypto.encoding_key(),
        expiration_minutes,
    )
    .expect("failed to mint test JWT")
}

/// Generate a user-bound CSRF token using the test crypto's CSRF secret, exactly
/// as the server does at session-issue time. Used to forge a token bound to a
/// *different* user id.
// Only the security integration binary uses this; `common` is compiled per binary.
#[allow(dead_code)]
pub fn mint_csrf_token(user_id: &str) -> String {
    let crypto = test_crypto();
    app::infrastructure::api::generate_csrf_token(crypto.csrf_secret(), user_id)
}

/// Build a fully wired test application from a sqlx-provided pool.
// Not every test binary uses this; `common` is compiled per binary.
#[allow(dead_code)]
pub fn create_test_app(pool: PgPool) -> Router {
    create_test_app_full(pool, console_email())
}

/// The email service every builder uses unless a test wants delivery to fail.
fn console_email() -> Arc<dyn EmailService> {
    Arc::new(ConsoleEmailService::new("Test".to_string()))
}

/// An email service whose first `fail_count` sends fail, after which it behaves
/// like [`ConsoleEmailService`].
///
/// Lets a test drive a mail outage and then its recovery in one run — the case
/// that matters is a registration whose email fails, followed by a resend that
/// works, which is only reachable if the failed send did not arm the cooldown.
// Not every test binary uses this; `common` is compiled per binary.
#[allow(dead_code)]
pub struct FailFirstEmailService {
    sends: AtomicUsize,
    fail_count: usize,
    inner: ConsoleEmailService,
}

#[allow(dead_code)]
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
}

/// Like [`create_test_app`] but with an injectable email service, so tests can
/// drive the paths where delivery fails.
// Not every test binary uses this; `common` is compiled per binary.
#[allow(dead_code)]
pub fn create_test_app_with_email_service(
    pool: PgPool,
    email_service: Arc<dyn EmailService>,
) -> Router {
    create_test_app_full(pool, email_service)
}

#[allow(clippy::too_many_lines)]
fn create_test_app_full(pool: PgPool, email_service: Arc<dyn EmailService>) -> Router {
    let crypto = test_crypto();

    let db = Arc::new(Database::from_pool(pool));

    let code_generator = Arc::new(RandomCodeGenerator::development());
    let jwt_service = Arc::new(CryptoJwtService::new(crypto.clone()));
    let password_hasher = Arc::new(Argon2Hasher::new(crypto.clone()));
    let token_service = Arc::new(CryptoTokenService::new(crypto.clone()));
    let turnstile_service: Arc<dyn TurnstileService> = Arc::new(NoOpTurnstileService);
    let mx_validator: Arc<dyn MxValidator> = Arc::new(NoOpMxValidator);

    let user_repo = Arc::new(PostgresUserRepository::new(db.clone()));
    let oauth_account_repo: Arc<dyn OAuthAccountRepository> =
        Arc::new(PostgresOAuthAccountRepository::new(db.clone()));
    let refresh_token_repo = Arc::new(PostgresRefreshTokenRepository::new(db.clone()));
    let email_verification_repo = Arc::new(PostgresEmailVerificationRepository::new(db.clone()));
    let password_reset_repo = Arc::new(PostgresPasswordResetRepository::new(db.clone()));
    let db_pool = db.pool().clone();

    let register_uc = Arc::new(RegisterUserUseCase::new(user_repo.clone()));
    let login_uc = Arc::new(LoginUserUseCase::new(user_repo.clone()));
    let get_current_user_uc = Arc::new(GetCurrentUserUseCase::new(user_repo.clone()));
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
        mx_validator,
        oauth_account_repo,
        crypto: crypto.clone(),
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
    });

    let oauth_deps = Arc::new(OAuthHandlerDeps {
        callback_use_case: oauth_callback_uc,
        unlink_use_case: oauth_unlink_uc,
        refresh_token_use_case: refresh_token_uc,
        oauth_clients: std::collections::HashMap::new(),
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
    });

    create_router(
        RouterDeps {
            auth: auth_deps,
            verification: verification_deps,
            oauth: oauth_deps,
            profile: profile_deps,
            admin: admin_deps,
            db_pool,
            crypto,
            users: user_repo,
        },
        "http://localhost:4321",
    )
}
