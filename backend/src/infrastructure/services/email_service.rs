//! Email service implementations: SMTP (production) and console (dev).

use std::error::Error as StdError;
use std::time::Duration;

use async_trait::async_trait;
use lettre::message::header::{Header, HeaderName, HeaderValue};
use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::application::ports::EmailService;
use crate::domain::errors::DomainError;
use crate::infrastructure::services::email_templates::{
    RenderedEmail, password_reset_email, verification_email,
};

/// Number of SMTP send attempts before giving up (1 initial + retries).
const MAX_SEND_ATTEMPTS: u32 = 3;
/// Base backoff between SMTP send attempts; doubles each retry.
const SEND_BACKOFF_BASE: Duration = Duration::from_millis(300);
/// Per-attempt SMTP timeout. lettre's default is 60s, which alone exceeds the
/// API's `DEFAULT_REQUEST_TIMEOUT` (30s): an unreachable relay then burned the
/// whole request budget and the caller got a 408 instead of a sent-or-logged
/// outcome. Sized so the FULL retry sequence still fits inside that budget
/// (3 * 8s + 0.3s + 0.6s = 24.9s), which keeps this safe even though
/// registration now sends off the request path.
const SEND_TIMEOUT: Duration = Duration::from_secs(8);
/// Implicit-TLS submission port (SMTPS). [`SmtpEmailService::new`] picks the TLS
/// mode from the port so the two can never disagree.
///
/// Prefer STARTTLS on 587: some cloud hosts block outbound 465 entirely, so
/// implicit-TLS submission cannot leave the server. Kept because the port→mode
/// mapping is what stops a future `SMTP_PORT` change from silently pairing a port
/// with the wrong TLS mode, which makes `POST /auth/register` hang until it times out.
const SMTPS_PORT: u16 = 465;

/// `List-Unsubscribe` header (RFC 2369).
///
/// lettre 0.11 has no typed header for this, so we provide a minimal one. Even
/// for transactional auth mail, a `mailto:` unsubscribe target improves
/// deliverability/spam scoring with major providers.
#[derive(Clone)]
struct ListUnsubscribe(String);

impl Header for ListUnsubscribe {
    fn name() -> HeaderName {
        HeaderName::new_from_ascii_str("List-Unsubscribe")
    }

    fn parse(s: &str) -> Result<Self, Box<dyn StdError + Send + Sync>> {
        Ok(Self(s.to_owned()))
    }

    fn display(&self) -> HeaderValue {
        HeaderValue::new(Self::name(), self.0.clone())
    }
}

/// Minimal transport seam over the concrete lettre `AsyncSmtpTransport`.
///
/// Exists so the retry/backoff loop in [`SmtpEmailService::send`] can be tested
/// with a fake transport (e.g. one that fails N times then succeeds) without a
/// real SMTP connection. The error is carried as a `String` because the retry
/// loop only needs the message for logging and the final error.
#[async_trait]
pub trait EmailTransport: Send + Sync {
    /// Sends one already-built message, returning the transport error text on
    /// failure.
    async fn send_email(&self, message: Message) -> Result<(), String>;
}

#[async_trait]
impl EmailTransport for AsyncSmtpTransport<Tokio1Executor> {
    async fn send_email(&self, message: Message) -> Result<(), String> {
        self.send(message)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// SMTP email service for production use.
///
/// Reads configuration from env vars:
/// - `SMTP_HOST`, `SMTP_PORT` (default 587), `SMTP_USERNAME`, `SMTP_PASSWORD`
/// - `SMTP_FROM_ADDRESS`, `SMTP_FROM_NAME` (optional)
///
/// `SMTP_PORT` also selects the TLS mode: [`SMTPS_PORT`] uses implicit TLS,
/// anything else uses STARTTLS.
///
/// Sends `multipart/alternative` (plaintext + HTML) with a `List-Unsubscribe`
/// header and bounded send-retry.
///
/// **Deliverability is a DNS prerequisite, not a code concern:** the domain of
/// `SMTP_FROM_ADDRESS` must publish SPF, DKIM, and DMARC records, or major
/// providers will spam-folder or reject these messages regardless of content.
pub struct SmtpEmailService {
    mailer: Box<dyn EmailTransport>,
    from: Mailbox,
    app_name: String,
    /// `mailto:` unsubscribe target derived from the from-address.
    list_unsubscribe: String,
}

impl SmtpEmailService {
    /// Creates a new SMTP service.
    ///
    /// # Errors
    ///
    /// Returns an error if the SMTP relay cannot be configured or the from
    /// address is invalid.
    pub fn new(
        host: &str,
        port: u16,
        username: String,
        password: String,
        from_address: &str,
        from_name: Option<&str>,
        app_name: String,
    ) -> Result<Self, anyhow::Error> {
        let creds = Credentials::new(username, password);
        // Derive the TLS mode from the port instead of hardcoding one. `relay()`
        // means IMPLICIT TLS (its own default port is 465); pinning it to 587 with
        // `.port()` sent a TLS ClientHello at a listener waiting to emit a
        // plaintext `220`, so the handshake hung until the timeout. Ports other
        // than 465 are STARTTLS submission, which is what `starttls_relay()` does.
        let builder = if port == SMTPS_PORT {
            AsyncSmtpTransport::<Tokio1Executor>::relay(host)?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)?
        };
        let mailer = builder
            .port(port)
            .timeout(Some(SEND_TIMEOUT))
            .credentials(creds)
            .build();

        Self::with_transport(Box::new(mailer), from_address, from_name, app_name)
    }

    /// Creates an SMTP service over an injected [`EmailTransport`]. The production
    /// path uses [`SmtpEmailService::new`] (which builds the real lettre
    /// transport); this exists so the retry/backoff loop can be tested with a
    /// fake transport.
    ///
    /// # Errors
    ///
    /// Returns an error if the from address is invalid.
    pub fn with_transport(
        mailer: Box<dyn EmailTransport>,
        from_address: &str,
        from_name: Option<&str>,
        app_name: String,
    ) -> Result<Self, anyhow::Error> {
        let from: Mailbox = match from_name {
            Some(name) => format!("{name} <{from_address}>").parse()?,
            None => from_address.parse()?,
        };

        let list_unsubscribe = format!("<mailto:{from_address}?subject=unsubscribe>");

        Ok(Self {
            mailer,
            from,
            app_name,
            list_unsubscribe,
        })
    }
}

#[async_trait]
impl EmailService for SmtpEmailService {
    #[tracing::instrument(name = "service.email.send_verification", skip(self, code, magic_link), fields(to = %to))]
    async fn send_verification_email(
        &self,
        to: &str,
        code: &str,
        first_name: &str,
        magic_link: Option<String>,
    ) -> Result<(), DomainError> {
        let rendered = verification_email(&self.app_name, first_name, code, magic_link.as_deref());
        self.send(to, rendered).await
    }

    #[tracing::instrument(name = "service.email.send_password_reset", skip(self, code), fields(to = %to))]
    async fn send_password_reset_email(
        &self,
        to: &str,
        code: &str,
        first_name: &str,
    ) -> Result<(), DomainError> {
        let rendered = password_reset_email(&self.app_name, first_name, code);
        self.send(to, rendered).await
    }
}

impl SmtpEmailService {
    /// Builds a `multipart/alternative` message (plaintext + HTML) and sends it
    /// with bounded retry/backoff so a transient SMTP blip doesn't fail the
    /// caller's flow.
    async fn send(&self, to: &str, rendered: RenderedEmail) -> Result<(), DomainError> {
        let to_mailbox: Mailbox = to
            .parse()
            .map_err(|_| DomainError::ExternalService(format!("Invalid to address: {to}")))?;

        let email = Message::builder()
            .from(self.from.clone())
            .to(to_mailbox)
            .subject(&rendered.subject)
            .header(ListUnsubscribe(self.list_unsubscribe.clone()))
            .multipart(MultiPart::alternative_plain_html(
                rendered.text.clone(),
                rendered.html.clone(),
            ))
            .map_err(|e| DomainError::ExternalService(format!("Failed to build email: {e}")))?;

        let mut backoff = SEND_BACKOFF_BASE;
        let mut last_err = String::new();
        for attempt in 1..=MAX_SEND_ATTEMPTS {
            match self.mailer.send_email(email.clone()).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last_err = e;
                    if attempt < MAX_SEND_ATTEMPTS {
                        tracing::warn!(
                            attempt,
                            max = MAX_SEND_ATTEMPTS,
                            error = %last_err,
                            "SMTP send failed; retrying after backoff"
                        );
                        tokio::time::sleep(backoff).await;
                        backoff *= 2;
                    }
                }
            }
        }

        Err(DomainError::ExternalService(format!(
            "SMTP send failed after {MAX_SEND_ATTEMPTS} attempts: {last_err}"
        )))
    }
}

/// A no-op email service for development/testing.
///
/// Logs emails to tracing instead of sending them.
pub struct ConsoleEmailService {
    app_name: String,
}

impl ConsoleEmailService {
    #[must_use]
    pub const fn new(app_name: String) -> Self {
        Self { app_name }
    }
}

#[async_trait]
impl EmailService for ConsoleEmailService {
    #[tracing::instrument(name = "service.email.send_verification_console", skip(self, code, magic_link), fields(to = %to))]
    async fn send_verification_email(
        &self,
        to: &str,
        code: &str,
        first_name: &str,
        magic_link: Option<String>,
    ) -> Result<(), DomainError> {
        tracing::info!(
            app = %self.app_name,
            to = %to,
            first_name = %first_name,
            code = %code,
            magic_link = magic_link.as_deref().unwrap_or("(none)"),
            "📧 VERIFICATION EMAIL (console mode)"
        );
        Ok(())
    }

    #[tracing::instrument(name = "service.email.send_password_reset_console", skip(self), fields(to = %to))]
    async fn send_password_reset_email(
        &self,
        to: &str,
        code: &str,
        first_name: &str,
    ) -> Result<(), DomainError> {
        tracing::info!(
            app = %self.app_name,
            to = %to,
            first_name = %first_name,
            code = %code,
            "📧 PASSWORD RESET EMAIL (console mode)"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::services::code_generator::is_test_email;

    // =========================================================================
    // ConsoleEmailService tests
    // =========================================================================

    #[tokio::test]
    async fn console_service_send_verification_email_succeeds() {
        let service = ConsoleEmailService::new("TestApp".to_string());

        let result = service
            .send_verification_email("user@example.com", "123456", "John", None)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn console_service_send_password_reset_email_succeeds() {
        let service = ConsoleEmailService::new("TestApp".to_string());

        let result = service
            .send_password_reset_email("user@example.com", "654321", "Jane")
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn console_service_works_with_different_app_names() {
        let service1 = ConsoleEmailService::new("App1".to_string());
        let service2 = ConsoleEmailService::new("Another App".to_string());

        assert!(
            service1
                .send_verification_email("test@example.com", "111111", "User1", None)
                .await
                .is_ok()
        );
        assert!(
            service2
                .send_verification_email("test@example.com", "222222", "User2", None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn console_service_handles_empty_first_name() {
        let service = ConsoleEmailService::new("TestApp".to_string());

        let result = service
            .send_verification_email("user@example.com", "123456", "", None)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn console_service_handles_special_characters_in_name() {
        let service = ConsoleEmailService::new("TestApp".to_string());

        let result = service
            .send_verification_email("user@example.com", "123456", "José María O'Brien", None)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn console_service_handles_unicode_name() {
        let service = ConsoleEmailService::new("TestApp".to_string());

        // Japanese name
        let result = service
            .send_verification_email("user@example.com", "123456", "田中太郎", None)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn console_service_handles_long_code() {
        let service = ConsoleEmailService::new("TestApp".to_string());

        // Edge case: longer than typical 6-digit code
        let result = service
            .send_verification_email("user@example.com", "12345678901234567890", "User", None)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn console_service_handles_empty_app_name() {
        let service = ConsoleEmailService::new(String::new());

        let result = service
            .send_verification_email("user@example.com", "123456", "John", None)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn console_service_password_reset_handles_special_chars() {
        let service = ConsoleEmailService::new("My App <test>".to_string());

        let result = service
            .send_password_reset_email("user@example.com", "999999", "User & Admin")
            .await;

        assert!(result.is_ok());
    }

    // =========================================================================
    // is_test_email tests (case-sensitive - only lowercase patterns)
    // =========================================================================

    #[test]
    fn is_test_email_detects_e2e_prefix() {
        assert!(is_test_email("e2e-test@example.com"));
        assert!(is_test_email("e2e-user123@domain.com"));
        // Note: is_test_email is case-sensitive, uppercase won't match
        assert!(!is_test_email("E2E-caps@example.com"));
    }

    #[test]
    fn is_test_email_detects_test_prefix() {
        assert!(is_test_email("test-user@example.com"));
        assert!(is_test_email("test-123@domain.com"));
        // Note: is_test_email is case-sensitive, uppercase won't match
        assert!(!is_test_email("TEST-upper@example.com"));
    }

    #[test]
    fn is_test_email_detects_playwright_prefix() {
        assert!(is_test_email("playwright@example.com"));
        assert!(is_test_email("playwright-user@domain.com"));
        // Note: is_test_email is case-sensitive, uppercase won't match
        assert!(!is_test_email("PLAYWRIGHT123@example.com"));
    }

    #[test]
    fn is_test_email_rejects_normal_emails() {
        assert!(!is_test_email("user@example.com"));
        assert!(!is_test_email("john.doe@company.com"));
        assert!(!is_test_email("testing@example.com")); // "testing" != "test-"
        assert!(!is_test_email("latest@example.com")); // contains "test" but not prefix
        assert!(!is_test_email("note2e@example.com")); // contains "e2e" but not prefix
    }

    #[test]
    fn is_test_email_handles_edge_cases() {
        assert!(!is_test_email(""));
        assert!(!is_test_email("@example.com"));
        assert!(is_test_email("test-@example.com")); // minimal test- prefix
    }

    // =========================================================================
    // SmtpEmailService construction + pre-network validation
    //
    // The actual SMTP send is a network operation (not HTTP, so wiremock can't
    // stand in), so we cover what runs before any connection: from-mailbox
    // construction, the derived List-Unsubscribe header, and the invalid-to
    // address guard inside `send`, which fails before the relay is dialed.
    // =========================================================================

    fn smtp_service() -> SmtpEmailService {
        SmtpEmailService::new(
            "smtp.example.com",
            587,
            "user".to_string(),
            "pass".to_string(),
            "noreply@example.com",
            Some("App"),
            "App".to_string(),
        )
        .expect("valid SMTP config builds")
    }

    #[test]
    fn smtp_new_with_from_name_builds_display_mailbox() {
        let service = smtp_service();
        assert_eq!(service.from.to_string(), "App <noreply@example.com>");
    }

    /// `DEFAULT_REQUEST_TIMEOUT` in `infrastructure/api/routes.rs`. The resend
    /// endpoint awaits this whole sequence inside a request, so it has to fit.
    /// (Registration awaits it too, but under the tighter
    /// `REGISTRATION_SEND_BUDGET` in `api/handlers/auth_handler.rs`.) Keep in
    /// step if either moves.
    const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

    /// The bug this guards: with no explicit timeout, lettre defaults to 60s — longer
    /// than the whole request budget on a single attempt — so an unreachable relay
    /// made `POST /auth/register` return 408 *after* committing the user row, instead
    /// of sending or logging a failure.
    #[test]
    fn full_send_retry_sequence_fits_inside_the_request_budget() {
        let backoff_total: Duration = (0..MAX_SEND_ATTEMPTS - 1)
            .map(|n| SEND_BACKOFF_BASE * 2u32.pow(n))
            .sum();
        let worst_case = SEND_TIMEOUT * MAX_SEND_ATTEMPTS + backoff_total;
        assert!(
            worst_case < DEFAULT_REQUEST_TIMEOUT,
            "worst-case send ({worst_case:?}) must fit inside DEFAULT_REQUEST_TIMEOUT ({DEFAULT_REQUEST_TIMEOUT:?})"
        );
    }

    /// Both TLS modes must build. `new` picks implicit TLS for [`SMTPS_PORT`] and
    /// STARTTLS otherwise; the original bug was one mode hardcoded for both ports,
    /// which dialed 587 with implicit TLS and hung the handshake until the timeout.
    #[test]
    fn smtp_new_builds_for_both_the_implicit_tls_and_starttls_ports() {
        for port in [SMTPS_PORT, 587] {
            let result = SmtpEmailService::new(
                "smtp.example.com",
                port,
                "user".to_string(),
                "pass".to_string(),
                "noreply@example.com",
                None,
                "App".to_string(),
            );
            assert!(result.is_ok(), "port {port} should build a transport");
        }
    }

    #[test]
    fn smtp_new_without_from_name_uses_bare_address() {
        let service = SmtpEmailService::new(
            "smtp.example.com",
            587,
            "user".to_string(),
            "pass".to_string(),
            "noreply@example.com",
            None,
            "App".to_string(),
        )
        .unwrap();
        assert_eq!(service.from.to_string(), "noreply@example.com");
    }

    #[test]
    fn smtp_new_derives_mailto_list_unsubscribe_from_address() {
        let service = smtp_service();
        assert_eq!(
            service.list_unsubscribe,
            "<mailto:noreply@example.com?subject=unsubscribe>"
        );
    }

    #[test]
    fn smtp_new_rejects_invalid_from_address() {
        let result = SmtpEmailService::new(
            "smtp.example.com",
            587,
            "user".to_string(),
            "pass".to_string(),
            "not-an-email",
            None,
            "App".to_string(),
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn smtp_send_verification_rejects_invalid_to_address_before_network() {
        // An unparseable recipient fails fast inside `send`, before any relay
        // connection is attempted, so this is deterministic and offline.
        let service = smtp_service();
        let err = service
            .send_verification_email("not-an-email", "123456", "Jo", None)
            .await
            .expect_err("invalid recipient must error");
        assert!(matches!(err, DomainError::ExternalService(_)));
        assert!(err.to_string().contains("Invalid to address"));
    }

    #[tokio::test]
    async fn smtp_send_password_reset_rejects_invalid_to_address() {
        let service = smtp_service();
        let err = service
            .send_password_reset_email("@@@", "123456", "Jo")
            .await
            .expect_err("invalid recipient must error");
        assert!(matches!(err, DomainError::ExternalService(_)));
    }

    #[test]
    fn list_unsubscribe_header_parses_and_displays_its_value() {
        // Exercises the Header impl (name/parse/display) for the custom header.
        let parsed = ListUnsubscribe::parse("<mailto:x@y?subject=unsubscribe>").unwrap();
        assert_eq!(
            AsRef::<str>::as_ref(&ListUnsubscribe::name()),
            "List-Unsubscribe"
        );
        assert_eq!(parsed.0, "<mailto:x@y?subject=unsubscribe>");
        let header = ListUnsubscribe("<mailto:a@b>".to_string());
        // display() round-trips the stored value into a HeaderValue.
        assert!(format!("{:?}", header.display()).contains("mailto:a@b"));
    }

    // =========================================================================
    // Retry/backoff loop coverage via an injected fake EmailTransport.
    //
    // Tests use `start_paused` so the backoff sleeps auto-advance virtual time,
    // keeping them fast and deterministic without a real SMTP connection.
    // =========================================================================
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Fake transport that fails its first `fail_first` calls, then succeeds, and
    /// records how many times it was invoked.
    struct FlakyTransport {
        calls: Arc<AtomicU32>,
        fail_first: u32,
    }

    #[async_trait]
    impl EmailTransport for FlakyTransport {
        async fn send_email(&self, _message: Message) -> Result<(), String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= self.fail_first {
                Err(format!("transient failure #{n}"))
            } else {
                Ok(())
            }
        }
    }

    fn service_with(transport: Box<dyn EmailTransport>) -> SmtpEmailService {
        SmtpEmailService::with_transport(
            transport,
            "noreply@example.com",
            Some("App"),
            "App".to_string(),
        )
        .expect("valid from address builds")
    }

    #[tokio::test(start_paused = true)]
    async fn send_retries_then_succeeds() {
        // Fails MAX-1 times, then succeeds on the final allowed attempt.
        let calls = Arc::new(AtomicU32::new(0));
        let service = service_with(Box::new(FlakyTransport {
            calls: Arc::clone(&calls),
            fail_first: MAX_SEND_ATTEMPTS - 1,
        }));

        let result = service
            .send_verification_email("user@example.com", "123456", "Jo", None)
            .await;

        assert!(result.is_ok(), "should eventually succeed: {result:?}");
        assert_eq!(calls.load(Ordering::SeqCst), MAX_SEND_ATTEMPTS);
    }

    #[tokio::test(start_paused = true)]
    async fn send_exhausts_retries_and_returns_terminal_error() {
        let calls = Arc::new(AtomicU32::new(0));
        let service = service_with(Box::new(FlakyTransport {
            calls: Arc::clone(&calls),
            fail_first: u32::MAX, // always fail
        }));

        let err = service
            .send_password_reset_email("user@example.com", "123456", "Jo")
            .await
            .expect_err("exhausted retries must error");

        assert!(matches!(err, DomainError::ExternalService(_)));
        let msg = err.to_string();
        assert!(
            msg.contains(&format!(
                "SMTP send failed after {MAX_SEND_ATTEMPTS} attempts"
            )),
            "unexpected error: {msg}"
        );
        // Carries the last transport error text.
        assert!(msg.contains("transient failure"));
        assert_eq!(calls.load(Ordering::SeqCst), MAX_SEND_ATTEMPTS);
    }

    /// Records the recipient/subject of the single message it is handed.
    struct RecordingTransport {
        recipients: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl EmailTransport for RecordingTransport {
        async fn send_email(&self, message: Message) -> Result<(), String> {
            // Render the built multipart message to bytes and confirm it carries
            // the recipient header, proving the message was assembled and handed
            // to the transport.
            let raw = String::from_utf8_lossy(&message.formatted()).into_owned();
            self.recipients.lock().unwrap().push(raw);
            Ok(())
        }
    }

    #[tokio::test]
    async fn valid_recipient_builds_and_sends_message_to_transport() {
        let recipients = Arc::new(std::sync::Mutex::new(Vec::new()));
        let service = service_with(Box::new(RecordingTransport {
            recipients: Arc::clone(&recipients),
        }));

        service
            .send_verification_email("alice@example.com", "654321", "Alice", None)
            .await
            .expect("send should succeed");

        let captured = std::mem::take(&mut *recipients.lock().unwrap());
        assert_eq!(captured.len(), 1, "transport called exactly once");
        let raw = &captured[0];
        assert!(raw.contains("alice@example.com"), "recipient in message");
        assert!(raw.contains("noreply@example.com"), "from in message");
        // multipart/alternative wrapper is present.
        assert!(raw.contains("multipart/alternative"));
        assert!(raw.contains("List-Unsubscribe"));
    }
}
