//! Shared, dependency-free utilities usable from any layer.
//!
//! - [`fingerprint`] - Error fingerprinting for grouping similar errors
//! - [`redact`] - Field-name and URL-query redaction rules for telemetry
//!
//! This crate is a leaf: it depends on no other workspace crate. Key material
//! and cryptographic primitives live in `platform::crypto`, since they are
//! built from platform settings.

pub mod fingerprint;
pub mod redact;
