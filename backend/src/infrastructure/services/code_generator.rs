//! Verification code generator implementations.

use crate::application::ports::CodeGenerator;
use rand::RngExt;

/// Test email patterns that receive fixed codes (only in dev mode).
///
/// Any email matching these patterns will receive "123456" as their code.
const TEST_EMAIL_PATTERNS: &[&str] = &[
    "e2e-",       // e2e-test@example.com, e2e-user@example.com
    "test-",      // test-user@example.com
    "playwright", // playwright@example.com
];

/// Fixed code used for test emails.
pub const TEST_CODE: &str = "123456";

/// Checks if an email matches a test pattern.
///
/// This is used by the email service to skip sending emails to test addresses.
/// Returns true if the email starts with any of the test patterns.
#[must_use]
pub fn is_test_email(email: &str) -> bool {
    let local_part = email.split('@').next().unwrap_or("");
    TEST_EMAIL_PATTERNS
        .iter()
        .any(|pattern| local_part.starts_with(pattern))
}

/// Production code generator that uses random 6-digit codes.
///
/// In dev mode only (`debug_assertions`), emails matching test patterns
/// (e.g., `e2e-*`, `test-*`) receive a fixed code to enable E2E testing.
/// In release builds, all emails receive random codes.
pub struct RandomCodeGenerator {
    /// Whether to allow test email patterns (dev mode only).
    allow_test_emails: bool,
}

impl RandomCodeGenerator {
    /// Creates a new code generator.
    ///
    /// Test email patterns are only enabled when `allow_test_emails` is true,
    /// which should only be set in development/test environments.
    #[must_use]
    pub const fn new(allow_test_emails: bool) -> Self {
        Self { allow_test_emails }
    }

    /// Creates a production code generator (no test emails).
    #[must_use]
    pub const fn production() -> Self {
        Self::new(false)
    }

    /// Creates a development code generator (test emails enabled).
    #[must_use]
    pub const fn development() -> Self {
        Self::new(true)
    }

    /// Checks if an email matches a test pattern.
    fn is_test_email(&self, email: &str) -> bool {
        if !self.allow_test_emails {
            return false;
        }

        let local_part = email.split('@').next().unwrap_or("");
        TEST_EMAIL_PATTERNS
            .iter()
            .any(|pattern| local_part.starts_with(pattern))
    }
}

impl Default for RandomCodeGenerator {
    fn default() -> Self {
        // Default to production mode (no test emails)
        Self::production()
    }
}

impl CodeGenerator for RandomCodeGenerator {
    fn generate_code(&self, email: &str) -> String {
        if self.is_test_email(email) {
            tracing::debug!(email = %email, "Using fixed test code for test email");
            return TEST_CODE.to_string();
        }

        let code: u32 = rand::rng().random_range(0..1_000_000);
        format!("{code:06}")
    }
}

/// Test-only code generator that always returns fixed codes.
///
/// Used in unit tests where we need predictable codes.
pub struct FixedCodeGenerator {
    code: String,
}

impl FixedCodeGenerator {
    #[must_use]
    pub fn new(code: &str) -> Self {
        Self {
            code: code.to_string(),
        }
    }
}

impl Default for FixedCodeGenerator {
    fn default() -> Self {
        Self::new(TEST_CODE)
    }
}

impl CodeGenerator for FixedCodeGenerator {
    fn generate_code(&self, _email: &str) -> String {
        self.code.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_random_generator_produces_6_digits() {
        let generator = RandomCodeGenerator::production();
        let code = generator.generate_code("user@example.com");
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_dev_generator_uses_fixed_code_for_e2e_email() {
        let generator = RandomCodeGenerator::development();
        assert_eq!(generator.generate_code("e2e-test@example.com"), TEST_CODE);
        assert_eq!(generator.generate_code("e2e-user@domain.com"), TEST_CODE);
    }

    #[test]
    fn test_dev_generator_uses_fixed_code_for_test_email() {
        let generator = RandomCodeGenerator::development();
        assert_eq!(generator.generate_code("test-user@example.com"), TEST_CODE);
    }

    #[test]
    fn test_dev_generator_uses_fixed_code_for_playwright_email() {
        let generator = RandomCodeGenerator::development();
        assert_eq!(generator.generate_code("playwright@example.com"), TEST_CODE);
    }

    #[test]
    fn test_production_generator_ignores_test_emails() {
        let generator = RandomCodeGenerator::production();
        // Run multiple times - at least one should differ from TEST_CODE
        let codes: Vec<_> = (0..10)
            .map(|_| generator.generate_code("e2e-test@example.com"))
            .collect();
        assert!(
            codes.iter().any(|c| c != TEST_CODE),
            "Production generator should use random codes for test emails"
        );
    }

    #[test]
    fn test_random_generator_uses_random_for_regular_email() {
        let generator = RandomCodeGenerator::development();
        // Run multiple times - at least one should differ from TEST_CODE
        let codes: Vec<_> = (0..10)
            .map(|_| generator.generate_code("user@example.com"))
            .collect();
        assert!(
            codes.iter().any(|c| c != TEST_CODE),
            "Random codes should not all equal TEST_CODE"
        );
    }

    #[test]
    fn test_fixed_generator_always_returns_same_code() {
        let generator = FixedCodeGenerator::new("999999");
        assert_eq!(generator.generate_code("any@email.com"), "999999");
        assert_eq!(generator.generate_code("another@email.com"), "999999");
    }

    #[test]
    fn test_is_test_email_e2e() {
        assert!(is_test_email("e2e-test@example.com"));
        assert!(is_test_email("e2e-user@domain.com"));
        assert!(is_test_email("e2e-1234567890@test.org"));
    }

    #[test]
    fn test_is_test_email_test_prefix() {
        assert!(is_test_email("test-user@example.com"));
        assert!(is_test_email("test-something@domain.com"));
    }

    #[test]
    fn test_is_test_email_playwright() {
        assert!(is_test_email("playwright@example.com"));
        assert!(is_test_email("playwright123@test.com"));
    }

    #[test]
    fn test_is_test_email_regular_emails() {
        assert!(!is_test_email("user@example.com"));
        assert!(!is_test_email("john.doe@company.com"));
        assert!(!is_test_email("testing@example.com")); // "testing" doesn't start with "test-"
        assert!(!is_test_email("note2e@example.com")); // doesn't start with "e2e-"
    }

    // =========================================================================
    // Property tests
    // =========================================================================

    /// Generates a test email local part (e2e-*, test-*, playwright*)
    fn test_email_local_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            "e2e-[a-z0-9]{1,20}".prop_map(|s| s),
            "test-[a-z0-9]{1,20}".prop_map(|s| s),
            "playwright[a-z0-9]{0,15}".prop_map(|s| s),
        ]
    }

    /// Generates a valid domain
    fn domain_strategy() -> impl Strategy<Value = String> {
        "[a-z]{2,10}\\.[a-z]{2,4}"
    }

    proptest! {
        /// Test emails matching patterns are detected
        #[test]
        fn prop_test_email_patterns_detected(
            local in test_email_local_strategy(),
            domain in domain_strategy()
        ) {
            let email = format!("{local}@{domain}");
            prop_assert!(
                is_test_email(&email),
                "Expected {email} to be detected as test email"
            );
        }

        /// Regular emails (not starting with test patterns) are not test emails
        #[test]
        fn prop_regular_emails_not_detected(
            // Local parts that don't start with test patterns
            local in "[a-z]{1}[a-z0-9]{2,15}",
            domain in domain_strategy()
        ) {
            // Filter out emails that would accidentally match patterns
            let local_lower = local.to_lowercase();
            prop_assume!(
                !local_lower.starts_with("e2e-") &&
                !local_lower.starts_with("test-") &&
                !local_lower.starts_with("playwright")
            );

            let email = format!("{local}@{domain}");
            prop_assert!(
                !is_test_email(&email),
                "Expected {email} to NOT be detected as test email"
            );
        }

        /// Generated codes are always 6 ASCII digits
        #[test]
        fn prop_generated_codes_are_6_digits(
            local in "[a-z]{3,10}",
            domain in domain_strategy()
        ) {
            let generator = RandomCodeGenerator::production();
            let email = format!("{local}@{domain}");
            let code = generator.generate_code(&email);

            prop_assert_eq!(code.len(), 6, "Code should be 6 characters");
            prop_assert!(
                code.chars().all(|c| c.is_ascii_digit()),
                "Code should only contain ASCII digits, got: {code}"
            );
        }

        /// Dev generator returns TEST_CODE for test emails
        #[test]
        fn prop_dev_generator_fixed_code_for_test_emails(
            local in test_email_local_strategy(),
            domain in domain_strategy()
        ) {
            let generator = RandomCodeGenerator::development();
            let email = format!("{local}@{domain}");
            let code = generator.generate_code(&email);

            prop_assert_eq!(code, TEST_CODE);
        }
    }
}
