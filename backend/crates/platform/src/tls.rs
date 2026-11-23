//! The process-wide TLS crypto provider: post-quantum key exchange first.
//!
//! Every outgoing TLS client in the backend (reqwest for Google and Turnstile,
//! lettre for SMTP) takes rustls's process default provider when one is
//! installed. [`install`] installs aws-lc-rs with hybrid ML-KEM-768 + X25519
//! (`X25519MLKEM768`) as the first key-exchange group, so traffic recorded
//! today can't be decrypted later by a quantum computer. Classical groups stay
//! as fallbacks for peers that don't support the hybrid. See OT-009 design D6.

use std::sync::Arc;

use rustls::NamedGroup;
use rustls::crypto::aws_lc_rs::{self, kx_group};
use rustls::crypto::{CryptoProvider, SupportedKxGroup};

/// The key-exchange group every connection offers first.
pub const PREFERRED_GROUP: NamedGroup = NamedGroup::X25519MLKEM768;

/// Key-exchange groups in preference order: the post-quantum hybrid, then
/// classical fallbacks.
#[must_use]
pub fn key_exchange_groups() -> Vec<&'static dyn SupportedKxGroup> {
    vec![
        kx_group::X25519MLKEM768,
        kx_group::X25519,
        kx_group::SECP256R1,
        kx_group::SECP384R1,
    ]
}

/// The aws-lc-rs provider with [`key_exchange_groups`] as its groups.
#[must_use]
pub fn provider() -> CryptoProvider {
    CryptoProvider {
        kx_groups: key_exchange_groups(),
        ..aws_lc_rs::default_provider()
    }
}

/// Install [`provider`] as the process default. Call once at startup, before
/// any TLS client is built.
///
/// # Errors
///
/// Returns an error if a different provider was already installed whose first
/// key-exchange group isn't [`PREFERRED_GROUP`].
pub fn install() -> Result<(), String> {
    match provider().install_default() {
        Ok(()) => Ok(()),
        Err(existing) => check_prefers_hybrid(&existing),
    }
}

/// Check that `provider` offers [`PREFERRED_GROUP`] first.
///
/// # Errors
///
/// Returns a message naming the group it offers first instead.
pub fn check_prefers_hybrid(provider: &Arc<CryptoProvider>) -> Result<(), String> {
    match provider.kx_groups.first().map(|g| g.name()) {
        Some(PREFERRED_GROUP) => Ok(()),
        other => Err(format!(
            "TLS provider offers {other:?} first, not {PREFERRED_GROUP:?} (post-quantum key exchange)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_offers_the_hybrid_first_with_classical_fallbacks() {
        let names: Vec<NamedGroup> = provider().kx_groups.iter().map(|g| g.name()).collect();
        assert_eq!(names[0], NamedGroup::X25519MLKEM768);
        assert!(names.contains(&NamedGroup::X25519));
        assert!(
            !names.contains(&NamedGroup::MLKEM768),
            "no PQ-only group: keep the hybrid"
        );
    }

    #[test]
    fn installing_leaves_a_hybrid_first_default() {
        install().expect("installs or finds a hybrid-first default");
        let installed = CryptoProvider::get_default().expect("a default is installed");
        assert!(check_prefers_hybrid(installed).is_ok());
    }

    #[test]
    fn a_classical_first_provider_is_reported() {
        let classical = Arc::new(CryptoProvider {
            kx_groups: vec![kx_group::X25519, kx_group::X25519MLKEM768],
            ..aws_lc_rs::default_provider()
        });
        let err = check_prefers_hybrid(&classical).unwrap_err();
        assert!(
            err.contains("X25519") && err.contains("X25519MLKEM768"),
            "{err}"
        );
    }
}
