//! Shared, dependency-free utilities usable from any layer.
//!
//! - [`fingerprint`] - Error fingerprinting for grouping similar errors
//!
//! This module is a leaf: it imports nothing from `domain`, `application`, or
//! `infrastructure`. Key material and cryptographic primitives live in
//! `infrastructure::crypto`, since they are built from infrastructure settings.

pub mod fingerprint;
