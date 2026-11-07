//! DNS MX record validation service.

use async_trait::async_trait;
use hickory_resolver::TokioResolver;
use hickory_resolver::net::NetError;

use crate::application::ports::MxValidator;
use crate::domain::errors::DomainError;

/// Outcome of a single MX lookup, with the error pre-classified.
///
/// Distinguishing "no records / NXDOMAIN" (→ `false`) from any other failure (→
/// [`DomainError::ExternalService`]) here lets the mapping logic in
/// [`HickoryMxValidator::has_mx_record`] be tested with a fake lookup.
#[derive(Debug)]
pub enum MxLookupError {
    /// The domain has no MX records, or does not exist.
    NoRecords,
    /// Any other resolver failure (timeout, SERVFAIL, etc.), carrying its text.
    Other(String),
}

/// Minimal DNS-lookup seam. The real impl wraps the hickory resolver; tests can
/// inject a fake to drive the has-records / no-records / error branches.
#[async_trait]
pub trait MxLookup: Send + Sync {
    /// Returns the MX exchange hostnames for `domain` (empty if none).
    async fn lookup_mx(&self, domain: &str) -> Result<Vec<String>, MxLookupError>;
}

/// hickory-backed [`MxLookup`] using the system resolver.
struct HickoryMxLookup {
    resolver: TokioResolver,
}

#[async_trait]
impl MxLookup for HickoryMxLookup {
    async fn lookup_mx(&self, domain: &str) -> Result<Vec<String>, MxLookupError> {
        match self.resolver.mx_lookup(domain).await {
            Ok(lookup) => Ok(lookup.answers().iter().map(ToString::to_string).collect()),
            Err(e) if e.is_no_records_found() || e.is_nx_domain() => Err(MxLookupError::NoRecords),
            Err(e) => Err(MxLookupError::Other(e.to_string())),
        }
    }
}

/// DNS-based MX validator using the system resolver.
pub struct HickoryMxValidator {
    lookup: Box<dyn MxLookup>,
}

impl HickoryMxValidator {
    /// Creates a new validator using the system DNS configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the system resolver cannot be initialised.
    pub fn new() -> Result<Self, NetError> {
        let resolver = TokioResolver::builder_tokio()?.build()?;
        Ok(Self::with_lookup(Box::new(HickoryMxLookup { resolver })))
    }

    /// Creates a validator over an injected [`MxLookup`]. The production path uses
    /// [`HickoryMxValidator::new`]; this exists so the error-mapping logic in
    /// [`HickoryMxValidator::has_mx_record`] can be tested with a fake lookup.
    #[must_use]
    pub fn with_lookup(lookup: Box<dyn MxLookup>) -> Self {
        Self { lookup }
    }
}

#[async_trait]
impl MxValidator for HickoryMxValidator {
    async fn has_mx_record(&self, domain: &str) -> Result<bool, DomainError> {
        match self.lookup.lookup_mx(domain).await {
            Ok(answers) => Ok(!answers.is_empty()),
            Err(MxLookupError::NoRecords) => Ok(false),
            Err(MxLookupError::Other(e)) => Err(DomainError::ExternalService(format!(
                "MX lookup failed for {domain}: {e}"
            ))),
        }
    }
}

/// A no-op MX validator that always returns true.
///
/// Use this when MX validation is disabled (e.g., in tests or local dev).
pub struct NoOpMxValidator;

#[async_trait]
impl MxValidator for NoOpMxValidator {
    async fn has_mx_record(&self, _domain: &str) -> Result<bool, DomainError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_validator_always_returns_true() {
        let validator = NoOpMxValidator;
        let result = validator.has_mx_record("anything.com").await.unwrap();
        assert!(result);
    }

    // `HickoryMxValidator` issues real DNS queries, so it is intentionally not
    // exercised in unit tests (no network/DNS in the unit suite). Coverage here
    // is limited to the no-op validator, which must accept any input without a
    // lookup.

    #[tokio::test]
    async fn noop_validator_accepts_empty_and_unusual_inputs() {
        let validator = NoOpMxValidator;
        assert!(validator.has_mx_record("").await.unwrap());
        assert!(
            validator
                .has_mx_record("no-such-tld.invalid")
                .await
                .unwrap()
        );
        assert!(validator.has_mx_record("UPPER.Example.COM").await.unwrap());
        assert!(
            validator
                .has_mx_record("sub.domain.example.org")
                .await
                .unwrap()
        );
    }

    // -------------------------------------------------------------------------
    // Error-mapping coverage via an injected fake `MxLookup`, driving each
    // branch of `HickoryMxValidator::has_mx_record` without real DNS.
    // -------------------------------------------------------------------------

    /// A fake lookup that returns a canned outcome regardless of `domain`.
    struct FakeLookup(std::sync::Mutex<Option<Result<Vec<String>, MxLookupError>>>);

    impl FakeLookup {
        fn new(outcome: Result<Vec<String>, MxLookupError>) -> Self {
            Self(std::sync::Mutex::new(Some(outcome)))
        }
    }

    #[async_trait]
    impl MxLookup for FakeLookup {
        async fn lookup_mx(&self, _domain: &str) -> Result<Vec<String>, MxLookupError> {
            self.0
                .lock()
                .unwrap()
                .take()
                .expect("FakeLookup outcome consumed once")
        }
    }

    fn validator_with(outcome: Result<Vec<String>, MxLookupError>) -> HickoryMxValidator {
        HickoryMxValidator::with_lookup(Box::new(FakeLookup::new(outcome)))
    }

    #[tokio::test]
    async fn records_present_yields_true() {
        let v = validator_with(Ok(vec!["mail.example.com.".to_string()]));
        assert!(v.has_mx_record("example.com").await.unwrap());
    }

    #[tokio::test]
    async fn empty_record_list_yields_false() {
        let v = validator_with(Ok(vec![]));
        assert!(!v.has_mx_record("example.com").await.unwrap());
    }

    #[tokio::test]
    async fn no_records_error_yields_false() {
        let v = validator_with(Err(MxLookupError::NoRecords));
        assert!(!v.has_mx_record("example.com").await.unwrap());
    }

    #[tokio::test]
    async fn other_error_maps_to_external_service() {
        let v = validator_with(Err(MxLookupError::Other("SERVFAIL".into())));
        let err = v.has_mx_record("example.com").await.unwrap_err();
        assert!(matches!(err, DomainError::ExternalService(_)));
        assert!(err.to_string().contains("SERVFAIL"));
        assert!(err.to_string().contains("example.com"));
    }
}
