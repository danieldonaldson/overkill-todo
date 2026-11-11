//! Use case for password login: credential verification.

use crate::application::ports::{PasswordHasher, UserRepository};
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Plaintext hashed once (with the configured Argon2 parameters) to produce the
/// dummy hash used for timing equalization.
const DUMMY_PASSWORD: &str = "timing-equalization-dummy";

/// Why a login attempt failed.
#[derive(Debug)]
pub enum LoginError {
    /// Unknown email, password-less account, or wrong password. Deliberately a
    /// single variant: a distinct answer per case would confirm that the email
    /// has an account.
    InvalidCredentials,
    /// Infrastructure failure (e.g. the user lookup failed).
    Domain(DomainError),
}

impl From<DomainError> for LoginError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

/// Use case for password login: looks the user up and verifies the password.
///
/// # Timing equalization
///
/// Every failure path performs one Argon2 verification, so an unknown email or a
/// password-less account (mid-signup, or OAuth-only) takes as long as a wrong
/// password. Without this, response time would reveal which emails have
/// accounts.
///
/// # Brute-force protection (deliberately not here)
///
/// There is intentionally no per-account `failed_login`/`locked_until` counter on
/// this path. Password login is meant to be throttled at the edge — a rate limit on
/// `/api/v1/auth/*` at your proxy plus Cloudflare Turnstile on the form — which suits
/// high-entropy, Argon2id-verified passwords. (The low-entropy 6-digit reset/verify
/// codes keep their own atomic in-app attempt cap; see `verify_email.rs`.)
///
/// Per-IP rate limiting of `/api/v1/auth/*` is done at the Traefik level, outside
/// this repo; the Caddy edge in `deploy/compose` and the k8s ingress here don't
/// throttle. Turnstile is required outside `DEV_MODE` (startup fails without
/// `TURNSTILE_SECRET_KEY`).
pub struct LoginUserUseCase {
    user_repository: Arc<dyn UserRepository>,
    password_hasher: Arc<dyn PasswordHasher>,
    /// Lazily computed with the real hasher so its cost matches a stored hash.
    dummy_hash: OnceCell<String>,
}

impl LoginUserUseCase {
    /// Creates a new instance with the given dependencies.
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        password_hasher: Arc<dyn PasswordHasher>,
    ) -> Self {
        Self {
            user_repository,
            password_hasher,
            dummy_hash: OnceCell::new(),
        }
    }

    /// Verifies `email` + `password` and returns the authenticated user.
    ///
    /// # Errors
    ///
    /// - [`LoginError::InvalidCredentials`] for an unknown email, an account with
    ///   no password, a wrong password, or a stored hash that fails to verify.
    /// - [`LoginError::Domain`] if the user lookup fails.
    #[tracing::instrument(name = "usecase.login_user", skip_all)]
    pub async fn execute(&self, email: &str, password: &str) -> Result<User, LoginError> {
        let user = self.user_repository.find_by_email(email).await?;

        // An unknown email and a password-less account (mid-signup, or
        // OAuth-only) fail like a wrong password, after the same Argon2 work.
        // A malformed stored hash also counts as a failed login, not a 500.
        let matches = if let Some(hash) = user.as_ref().and_then(User::password_hash) {
            self.password_hasher
                .verify(password, hash)
                .await
                .unwrap_or(false)
        } else {
            self.verify_dummy_password(password).await;
            false
        };

        match user {
            Some(user) if matches => Ok(user),
            _ => Err(LoginError::InvalidCredentials),
        }
    }

    /// Runs an Argon2 verification against the dummy hash and discards the result.
    async fn verify_dummy_password(&self, password: &str) {
        let dummy = self
            .dummy_hash
            .get_or_try_init(|| self.password_hasher.hash(DUMMY_PASSWORD))
            .await;
        if let Ok(dummy) = dummy {
            let _ = self.password_hasher.verify(password, dummy).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::password_hasher::MockPasswordHasher;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::value_objects::Email;

    const EMAIL: &str = "test@example.com";

    fn user(password_hash: Option<&str>) -> User {
        User::new(
            Email::new(EMAIL).unwrap(),
            password_hash.map(str::to_string),
            "John".to_string(),
            "Doe".to_string(),
        )
    }

    fn repo_returning(found: Option<User>) -> MockUserRepository {
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email()
            .times(1)
            .returning(move |_| Ok(found.clone()));
        repo
    }

    /// A hasher whose `verify` succeeds only for `password` against `"stored"`,
    /// and which expects exactly `dummy_verifies` dummy-hash verifications.
    fn hasher(dummy_verifies: usize) -> MockPasswordHasher {
        let mut hasher = MockPasswordHasher::new();
        hasher
            .expect_hash()
            .times(usize::from(dummy_verifies > 0))
            .returning(|_| Ok("dummy".to_string()));
        hasher
            .expect_verify()
            .withf(|_, hash| hash == "dummy")
            .times(dummy_verifies)
            .returning(|_, _| Ok(false));
        hasher
            .expect_verify()
            .withf(|_, hash| hash == "stored")
            .returning(|password, _| Ok(password == "correct"));
        hasher
    }

    fn use_case(repo: MockUserRepository, hasher: MockPasswordHasher) -> LoginUserUseCase {
        LoginUserUseCase::new(Arc::new(repo), Arc::new(hasher))
    }

    #[tokio::test]
    async fn correct_password_returns_user() {
        let uc = use_case(repo_returning(Some(user(Some("stored")))), hasher(0));

        let user = uc.execute(EMAIL, "correct").await.unwrap();

        assert_eq!(user.email().as_str(), EMAIL);
    }

    #[tokio::test]
    async fn wrong_password_is_invalid_credentials() {
        let uc = use_case(repo_returning(Some(user(Some("stored")))), hasher(0));

        let result = uc.execute(EMAIL, "wrong").await;

        assert!(matches!(result, Err(LoginError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn unknown_email_runs_dummy_verify_and_is_invalid_credentials() {
        let uc = use_case(repo_returning(None), hasher(1));

        let result = uc.execute("nobody@example.com", "correct").await;

        assert!(matches!(result, Err(LoginError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn passwordless_account_runs_dummy_verify_and_is_invalid_credentials() {
        let uc = use_case(repo_returning(Some(user(None))), hasher(1));

        let result = uc.execute(EMAIL, "correct").await;

        assert!(matches!(result, Err(LoginError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn dummy_hash_is_computed_once() {
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email().times(2).returning(|_| Ok(None));
        let mut hasher = MockPasswordHasher::new();
        hasher
            .expect_hash()
            .times(1)
            .returning(|_| Ok("dummy".to_string()));
        hasher.expect_verify().times(2).returning(|_, _| Ok(false));
        let uc = use_case(repo, hasher);

        let _ = uc.execute(EMAIL, "a").await;
        let _ = uc.execute(EMAIL, "b").await;
    }

    #[tokio::test]
    async fn verify_error_is_invalid_credentials() {
        let mut hasher = MockPasswordHasher::new();
        hasher
            .expect_verify()
            .returning(|_, _| Err(DomainError::Validation("malformed hash".into())));
        let uc = use_case(repo_returning(Some(user(Some("stored")))), hasher);

        let result = uc.execute(EMAIL, "correct").await;

        assert!(matches!(result, Err(LoginError::InvalidCredentials)));
    }

    #[tokio::test]
    async fn repository_error_is_propagated() {
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email()
            .returning(|_| Err(DomainError::Persistence("Database error".into())));
        let uc = use_case(repo, MockPasswordHasher::new());

        let result = uc.execute(EMAIL, "correct").await;

        assert!(matches!(
            result,
            Err(LoginError::Domain(DomainError::Persistence(_)))
        ));
    }
}
