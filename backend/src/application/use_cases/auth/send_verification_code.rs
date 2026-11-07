//! Use case for sending an email verification code.

use crate::application::ports::{
    CodeGenerator, EmailService, EmailVerificationRepository, PasswordHasher,
};
use crate::domain::entities::EmailVerificationCode;
use crate::domain::entities::User;
use crate::domain::entities::attempt_lockout::RESEND_COOLDOWN_MINUTES;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::{Email, UserId};
use chrono::{Duration as ChronoDuration, Utc};
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Backend path that consumes a magic-link token (`GET ...?token=`). The link is
/// rooted at the frontend base URL because the API is served same-origin behind
/// the same proxy (see the dashboard's `API_BASE = '/api/v1'`).
const VERIFY_MAGIC_PATH: &str = "/api/v1/auth/verify-magic";

/// Why a registration-time verification code never reached the user.
///
/// Both variants are fatal to the signup: with no code in hand there is no way
/// to reach `verify-email`, so neither can be reported as success.
#[derive(Debug)]
pub enum DispatchError {
    /// The code could not be generated or persisted. Nothing was sent.
    Issue(DomainError),
    /// The code is persisted but its email could not be delivered inside the
    /// caller's budget. The resend cooldown has been released, so the user can
    /// ask for another one immediately.
    Delivery(DomainError),
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Issue(e) => write!(f, "could not issue verification code: {e}"),
            Self::Delivery(e) => write!(f, "could not deliver verification code: {e}"),
        }
    }
}

impl From<DispatchError> for DomainError {
    fn from(err: DispatchError) -> Self {
        match err {
            // Persisting the code is our own database; failing to reach the mail
            // relay is somebody else's service. Keep that distinction so the API
            // maps them to different statuses.
            DispatchError::Issue(e) => e,
            DispatchError::Delivery(e) => Self::ExternalService(e.to_string()),
        }
    }
}

/// A verification code that is already persisted but whose email has not been
/// sent yet.
///
/// Produced by [`SendVerificationCodeUseCase::issue_code`] and consumed by
/// [`SendVerificationCodeUseCase::send_pending`]; it carries the raw code and
/// magic-link token, which exist in plaintext nowhere else (the database holds
/// only their hashes).
pub struct PendingVerificationEmail {
    email: String,
    code: String,
    first_name: String,
    magic_link: String,
}

/// Use case for sending an email verification code.
///
/// Generates a 6-digit code, hashes it with Argon2, stores it in the database,
/// and sends it to the user via email. A one-click magic-link token is issued
/// alongside the code: a random UUID stored SHA-256-hashed (it is high-entropy,
/// so SHA-256 rather than Argon2), with the raw token embedded in the email link
/// and consumed by [`VerifyMagicLinkUseCase`].
pub struct SendVerificationCodeUseCase {
    email_verification_repo: Arc<dyn EmailVerificationRepository>,
    email_service: Arc<dyn EmailService>,
    code_generator: Arc<dyn CodeGenerator>,
    password_hasher: Arc<dyn PasswordHasher>,
    /// Frontend base URL the magic link is rooted at (API is same-origin).
    frontend_base_url: String,
}

impl SendVerificationCodeUseCase {
    /// Creates a new instance with the given dependencies.
    pub fn new(
        email_verification_repo: Arc<dyn EmailVerificationRepository>,
        email_service: Arc<dyn EmailService>,
        code_generator: Arc<dyn CodeGenerator>,
        password_hasher: Arc<dyn PasswordHasher>,
        frontend_base_url: String,
    ) -> Self {
        Self {
            email_verification_repo,
            email_service,
            code_generator,
            password_hasher,
            frontend_base_url,
        }
    }

    /// Builds the absolute magic-link URL for a raw token.
    fn magic_link(&self, token: &str) -> String {
        format!(
            "{base}{path}?token={token}",
            base = self.frontend_base_url.trim_end_matches('/'),
            path = VERIFY_MAGIC_PATH,
        )
    }

    /// Issues a fresh code + magic-link token for `user_id` and persists them,
    /// returning everything the email still needs.
    ///
    /// Split out from the send so a caller can put the persistence inside its
    /// request and the SMTP round-trip outside it — the code row must exist
    /// before the response lands, because the client may call `verify-email`
    /// immediately.
    async fn persist_code(
        &self,
        user_id: UserId,
        email: &Email,
        first_name: &str,
    ) -> Result<PendingVerificationEmail, DomainError> {
        // 6-digit code, Argon2-hashed (guessable → Argon2).
        let code = self.code_generator.generate_code(email.as_str());
        let code_hash = self.password_hasher.hash(&code).await?;

        // Magic-link token: random UUID, SHA-256-hashed for storage.
        let magic_token = Uuid::new_v4().to_string();
        let magic_token_hash = hex::encode(Sha256::digest(magic_token.as_bytes()));

        let verification_code =
            EmailVerificationCode::new_with_magic_token(user_id, code_hash, magic_token_hash);
        self.email_verification_repo
            .save(&verification_code)
            .await?;

        Ok(PendingVerificationEmail {
            email: email.as_str().to_string(),
            code,
            first_name: first_name.to_string(),
            magic_link: self.magic_link(&magic_token),
        })
    }

    /// Issues a fresh code + magic-link token for `user_id`, persists them, and
    /// emails both the code and the one-click link.
    async fn issue(
        &self,
        user_id: UserId,
        email: &Email,
        first_name: &str,
    ) -> Result<(), DomainError> {
        let pending = self.persist_code(user_id, email, first_name).await?;
        self.send_pending(pending).await
    }

    /// Issues and persists a code for `user` WITHOUT sending its email.
    ///
    /// Pair with [`Self::send_pending`]. Callers that must not put an SMTP
    /// round-trip inside a request budget await this, then hand the returned
    /// value to a spawned task.
    ///
    /// # Errors
    /// Returns an error if code hashing or persisting the code fails.
    #[tracing::instrument(
        name = "usecase.issue_verification_code",
        skip(self, user),
        fields(user_id = %user.id())
    )]
    pub async fn issue_code(&self, user: &User) -> Result<PendingVerificationEmail, DomainError> {
        self.persist_code(*user.id(), user.email(), user.first_name())
            .await
    }

    /// Emails an already-persisted code and its one-click link.
    ///
    /// # Errors
    /// Returns an error if the email service fails to send.
    #[tracing::instrument(name = "usecase.send_verification_email", skip(self, pending))]
    pub async fn send_pending(&self, pending: PendingVerificationEmail) -> Result<(), DomainError> {
        self.email_service
            .send_verification_email(
                &pending.email,
                &pending.code,
                &pending.first_name,
                Some(pending.magic_link),
            )
            .await
    }

    /// Issues a code and delivers it, giving the send at most `budget`.
    ///
    /// This is registration's entry point, and unlike [`Self::execute`] it
    /// reports delivery failure instead of swallowing it. The verification email
    /// is not a notification — it is the only channel carrying the code, and
    /// there is no password or session yet — so a signup whose email never
    /// leaves the building has failed, and saying otherwise just strands the
    /// user on a "check your email" screen with nothing to check.
    ///
    /// `budget` caps the whole send, retries included, so a dead relay cannot
    /// hold the request open for the transport's full retry sequence. A blip
    /// that fails fast still gets retried inside the budget and never surfaces.
    ///
    /// On failure the resend cooldown is released: `last_sent_at` is stamped at
    /// issue time, so leaving it would bar the user from re-requesting a code
    /// for [`RESEND_COOLDOWN_MINUTES`] — blocking the only recovery path with a
    /// send that never happened.
    ///
    /// # Errors
    /// [`DispatchError::Issue`] if the code could not be persisted;
    /// [`DispatchError::Delivery`] if it was persisted but not delivered.
    #[tracing::instrument(
        name = "usecase.issue_and_deliver_verification_code",
        skip(self, user),
        fields(user_id = %user.id())
    )]
    pub async fn issue_and_deliver(
        &self,
        user: &User,
        budget: Duration,
    ) -> Result<(), DispatchError> {
        let pending = self.issue_code(user).await.map_err(DispatchError::Issue)?;

        let sent = match tokio::time::timeout(budget, self.send_pending(pending)).await {
            Ok(result) => result,
            Err(_elapsed) => Err(DomainError::ExternalService(format!(
                "verification email not sent within {budget:?}"
            ))),
        };

        if let Err(e) = sent {
            self.release_resend_cooldown(user.id()).await;
            return Err(DispatchError::Delivery(e));
        }

        Ok(())
    }

    /// Back-dates `last_sent_at` past the cooldown so a user whose email failed
    /// can immediately request another.
    ///
    /// Best-effort: the caller is already returning a delivery error, and
    /// failing to widen the recovery window must not replace that with a
    /// different one.
    async fn release_resend_cooldown(&self, user_id: &UserId) {
        let released = Utc::now() - ChronoDuration::minutes(RESEND_COOLDOWN_MINUTES);
        if let Err(e) = self
            .email_verification_repo
            .set_last_sent_at(user_id, released)
            .await
        {
            tracing::error!(
                user_id = %user_id,
                error = %e,
                "Failed to release the resend cooldown after a delivery failure; \
                 the user may have to wait it out before requesting a new code"
            );
        }
    }

    /// Sends a verification code to the given user.
    ///
    /// This is called after user registration to initiate email verification.
    ///
    /// # Errors
    /// Returns an error if code hashing or repository operations fail.
    #[tracing::instrument(
        name = "usecase.send_verification_code",
        skip(self, user),
        fields(user_id = %user.id())
    )]
    pub async fn execute(&self, user: &User) -> Result<(), DomainError> {
        self.issue(*user.id(), user.email(), user.first_name())
            .await
    }

    /// Sends a verification code to a user by ID and email.
    ///
    /// Used for resending codes when we only have the user ID.
    ///
    /// # Errors
    /// Returns an error if code hashing or repository operations fail.
    #[tracing::instrument(
        name = "usecase.send_verification_code_by_id",
        skip(self),
        fields(user_id = %user_id)
    )]
    pub async fn execute_for_user(
        &self,
        user_id: UserId,
        email: Email,
        first_name: String,
    ) -> Result<(), DomainError> {
        self.issue(user_id, &email, &first_name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::code_generator::MockCodeGenerator;
    use crate::application::ports::outgoing::email_service::MockEmailService;
    use crate::application::ports::outgoing::email_verification_repository::MockEmailVerificationRepository;
    use crate::application::ports::outgoing::password_hasher::MockPasswordHasher;

    fn create_test_user() -> User {
        User::new(
            Email::new("test@example.com").unwrap(),
            None,
            "Test".to_string(),
            "User".to_string(),
        )
    }

    /// A generous budget for tests whose sends resolve immediately.
    const TEST_BUDGET: Duration = Duration::from_secs(5);

    /// Stands in for a relay that accepts the connection and then goes quiet —
    /// the failure mode that has no natural end and must be cut off by the
    /// caller's budget. `MockEmailService` can't express it: mockall resolves
    /// the async for us, so the return value is always ready.
    struct HangingEmailService;

    #[async_trait::async_trait]
    impl EmailService for HangingEmailService {
        async fn send_verification_email(
            &self,
            _to: &str,
            _code: &str,
            _first_name: &str,
            _magic_link: Option<String>,
        ) -> Result<(), DomainError> {
            // Comfortably longer than any budget under test, and longer than the
            // transport's own retry sequence (~24.9s) so nothing but the
            // caller's timeout can end it.
            tokio::time::sleep(Duration::from_secs(59)).await;
            Ok(())
        }

        async fn send_password_reset_email(
            &self,
            _to: &str,
            _code: &str,
            _first_name: &str,
        ) -> Result<(), DomainError> {
            unreachable!("verification-code tests never send a reset email")
        }
    }

    fn hasher_returning_hash() -> MockPasswordHasher {
        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher
            .expect_hash()
            .returning(|_| Ok("hashed_code".to_string()));
        mock_hasher
    }

    fn code_gen_returning(code: &'static str) -> MockCodeGenerator {
        let mut mock_code_gen = MockCodeGenerator::new();
        mock_code_gen
            .expect_generate_code()
            .returning(move |_| code.to_string());
        mock_code_gen
    }

    /// The happy path leaves the cooldown alone: `last_sent_at` was stamped at
    /// issue time and the email really did go out, so it is accurate.
    #[tokio::test]
    async fn issue_and_deliver_sends_and_keeps_the_cooldown_on_success() {
        let mut mock_repo = MockEmailVerificationRepository::new();
        mock_repo.expect_save().times(1).returning(|_| Ok(()));
        mock_repo.expect_set_last_sent_at().times(0);

        let mut mock_email = MockEmailService::new();
        mock_email
            .expect_send_verification_email()
            .times(1)
            .returning(|_, _, _, _| Ok(()));

        let use_case = SendVerificationCodeUseCase::new(
            Arc::new(mock_repo),
            Arc::new(mock_email),
            Arc::new(code_gen_returning("123456")),
            Arc::new(hasher_returning_hash()),
            "http://localhost:4321".to_string(),
        );

        let result = use_case
            .issue_and_deliver(&create_test_user(), TEST_BUDGET)
            .await;
        assert!(result.is_ok());
    }

    /// A failed send must be reported, not swallowed — the code is the only way
    /// to finish signup — and must not leave the resend cooldown armed, since
    /// resending is the sole recovery path and nothing was actually sent.
    #[tokio::test]
    async fn issue_and_deliver_reports_failure_and_releases_the_cooldown() {
        let mut mock_repo = MockEmailVerificationRepository::new();
        mock_repo.expect_save().times(1).returning(|_| Ok(()));
        mock_repo
            .expect_set_last_sent_at()
            .times(1)
            .withf(|_, last_sent_at| {
                // Back-dated far enough that `can_resend()` is already true.
                *last_sent_at <= Utc::now() - ChronoDuration::minutes(RESEND_COOLDOWN_MINUTES)
            })
            .returning(|_, _| Ok(()));

        let mut mock_email = MockEmailService::new();
        mock_email
            .expect_send_verification_email()
            .times(1)
            .returning(|_, _, _, _| Err(DomainError::ExternalService("relay unreachable".into())));

        let use_case = SendVerificationCodeUseCase::new(
            Arc::new(mock_repo),
            Arc::new(mock_email),
            Arc::new(code_gen_returning("123456")),
            Arc::new(hasher_returning_hash()),
            "http://localhost:4321".to_string(),
        );

        let result = use_case
            .issue_and_deliver(&create_test_user(), TEST_BUDGET)
            .await;
        assert!(matches!(result, Err(DispatchError::Delivery(_))));
    }

    /// A relay that accepts the connection and then never answers must not hold
    /// the request open for the transport's full retry sequence.
    #[tokio::test(start_paused = true)]
    async fn issue_and_deliver_gives_up_on_a_hanging_relay_at_the_budget() {
        let mut mock_repo = MockEmailVerificationRepository::new();
        mock_repo.expect_save().times(1).returning(|_| Ok(()));
        mock_repo
            .expect_set_last_sent_at()
            .times(1)
            .returning(|_, _| Ok(()));

        let use_case = SendVerificationCodeUseCase::new(
            Arc::new(mock_repo),
            Arc::new(HangingEmailService),
            Arc::new(code_gen_returning("123456")),
            Arc::new(hasher_returning_hash()),
            "http://localhost:4321".to_string(),
        );

        let result = use_case
            .issue_and_deliver(&create_test_user(), Duration::from_secs(10))
            .await;
        assert!(matches!(result, Err(DispatchError::Delivery(_))));
    }

    /// If the code never reached the database there is nothing to deliver and
    /// nothing to un-stamp — the caller must see an issue failure, not a
    /// delivery one, because the two map to different HTTP statuses.
    #[tokio::test]
    async fn issue_and_deliver_reports_an_issue_failure_without_sending() {
        let mut mock_repo = MockEmailVerificationRepository::new();
        mock_repo
            .expect_save()
            .times(1)
            .returning(|_| Err(DomainError::Persistence("save boom".into())));
        mock_repo.expect_set_last_sent_at().times(0);

        let mut mock_email = MockEmailService::new();
        mock_email.expect_send_verification_email().times(0);

        let use_case = SendVerificationCodeUseCase::new(
            Arc::new(mock_repo),
            Arc::new(mock_email),
            Arc::new(code_gen_returning("123456")),
            Arc::new(hasher_returning_hash()),
            "http://localhost:4321".to_string(),
        );

        let result = use_case
            .issue_and_deliver(&create_test_user(), TEST_BUDGET)
            .await;
        assert!(matches!(result, Err(DispatchError::Issue(_))));
    }

    /// The two failure kinds must stay distinguishable once mapped outward: our
    /// own database failing is not the same as the mail relay failing.
    #[test]
    fn dispatch_errors_map_to_distinct_domain_errors() {
        assert!(matches!(
            DomainError::from(DispatchError::Issue(DomainError::Persistence("x".into()))),
            DomainError::Persistence(_)
        ));
        assert!(matches!(
            DomainError::from(DispatchError::Delivery(DomainError::ExternalService(
                "x".into()
            ))),
            DomainError::ExternalService(_)
        ));
    }

    #[tokio::test]
    async fn test_send_verification_code_success() {
        let mut mock_repo = MockEmailVerificationRepository::new();
        mock_repo.expect_save().times(1).returning(|_| Ok(()));

        let mut mock_email = MockEmailService::new();
        mock_email
            .expect_send_verification_email()
            .times(1)
            .withf(|_, _, _, magic_link: &Option<String>| magic_link.is_some())
            .returning(|_, _, _, _| Ok(()));

        let mut mock_code_gen = MockCodeGenerator::new();
        mock_code_gen
            .expect_generate_code()
            .times(1)
            .returning(|_| "123456".to_string());

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher
            .expect_hash()
            .times(1)
            .returning(|_| Ok("hashed_code".to_string()));

        let use_case = SendVerificationCodeUseCase::new(
            Arc::new(mock_repo),
            Arc::new(mock_email),
            Arc::new(mock_code_gen),
            Arc::new(mock_hasher),
            "http://localhost:4321".to_string(),
        );

        let user = create_test_user();
        let result = use_case.execute(&user).await;

        assert!(result.is_ok());
    }

    /// `issue_code` must persist the code WITHOUT sending anything: registration
    /// awaits it inside the request and spawns only the send, so `verify-email`
    /// can be called the moment the response lands. When the persistence was
    /// deferred too, the code row raced the request that reads it and every
    /// register-then-verify test failed.
    #[tokio::test]
    async fn issue_code_persists_the_code_without_sending_the_email() {
        let mut mock_repo = MockEmailVerificationRepository::new();
        mock_repo.expect_save().times(1).returning(|_| Ok(()));

        let mut mock_email = MockEmailService::new();
        mock_email.expect_send_verification_email().times(0);

        let mut mock_code_gen = MockCodeGenerator::new();
        mock_code_gen
            .expect_generate_code()
            .times(1)
            .returning(|_| "123456".to_string());

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher
            .expect_hash()
            .times(1)
            .returning(|_| Ok("hashed_code".to_string()));

        let use_case = SendVerificationCodeUseCase::new(
            Arc::new(mock_repo),
            Arc::new(mock_email),
            Arc::new(mock_code_gen),
            Arc::new(mock_hasher),
            "http://localhost:4321".to_string(),
        );

        let user = create_test_user();
        let pending = use_case.issue_code(&user).await.expect("issue_code");

        assert_eq!(pending.code, "123456");
        assert_eq!(pending.email, "test@example.com");
        assert!(
            pending.magic_link.starts_with("http://localhost:4321"),
            "got: {}",
            pending.magic_link
        );
    }

    /// The deferred half still sends exactly what `issue_code` produced.
    #[tokio::test]
    async fn send_pending_emails_the_already_issued_code() {
        let mut mock_email = MockEmailService::new();
        mock_email
            .expect_send_verification_email()
            .times(1)
            .withf(|email, code, _, magic_link: &Option<String>| {
                email == "test@example.com" && code == "123456" && magic_link.is_some()
            })
            .returning(|_, _, _, _| Ok(()));

        let use_case = SendVerificationCodeUseCase::new(
            Arc::new(MockEmailVerificationRepository::new()),
            Arc::new(mock_email),
            Arc::new(MockCodeGenerator::new()),
            Arc::new(MockPasswordHasher::new()),
            "http://localhost:4321".to_string(),
        );

        let pending = PendingVerificationEmail {
            email: "test@example.com".to_string(),
            code: "123456".to_string(),
            first_name: "Test".to_string(),
            magic_link: "http://localhost:4321/api/v1/auth/verify-magic?token=t".to_string(),
        };

        assert!(use_case.send_pending(pending).await.is_ok());
    }

    #[tokio::test]
    async fn test_execute_for_user_success() {
        // The by-user-id resend entry point issues + persists a code and emails it,
        // exactly like `execute`, but takes the id/email/name directly.
        let mut mock_repo = MockEmailVerificationRepository::new();
        mock_repo.expect_save().times(1).returning(|_| Ok(()));

        let mut mock_email = MockEmailService::new();
        mock_email
            .expect_send_verification_email()
            .times(1)
            .withf(|_, _, _, magic_link: &Option<String>| magic_link.is_some())
            .returning(|_, _, _, _| Ok(()));

        let mut mock_code_gen = MockCodeGenerator::new();
        mock_code_gen
            .expect_generate_code()
            .times(1)
            .returning(|_| "123456".to_string());

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher
            .expect_hash()
            .times(1)
            .returning(|_| Ok("hashed_code".to_string()));

        let use_case = SendVerificationCodeUseCase::new(
            Arc::new(mock_repo),
            Arc::new(mock_email),
            Arc::new(mock_code_gen),
            Arc::new(mock_hasher),
            "http://localhost:4321".to_string(),
        );

        let result = use_case
            .execute_for_user(
                UserId::new(),
                Email::new("resend@example.com").unwrap(),
                "Resend".to_string(),
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_for_user_propagates_persistence_error() {
        // If persisting the freshly issued code fails, the error bubbles up and
        // no email is sent.
        let mut mock_repo = MockEmailVerificationRepository::new();
        mock_repo
            .expect_save()
            .times(1)
            .returning(|_| Err(DomainError::Persistence("save boom".into())));

        let mut mock_email = MockEmailService::new();
        mock_email.expect_send_verification_email().times(0);

        let mut mock_code_gen = MockCodeGenerator::new();
        mock_code_gen
            .expect_generate_code()
            .times(1)
            .returning(|_| "123456".to_string());

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher
            .expect_hash()
            .times(1)
            .returning(|_| Ok("hashed_code".to_string()));

        let use_case = SendVerificationCodeUseCase::new(
            Arc::new(mock_repo),
            Arc::new(mock_email),
            Arc::new(mock_code_gen),
            Arc::new(mock_hasher),
            "http://localhost:4321".to_string(),
        );

        let result = use_case
            .execute_for_user(
                UserId::new(),
                Email::new("resend-fail@example.com").unwrap(),
                "Resend".to_string(),
            )
            .await;

        assert!(matches!(result, Err(DomainError::Persistence(_))));
    }
}
