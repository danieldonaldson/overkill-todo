//! Backend crate (hexagonal architecture): domain → application → infrastructure.
// `#[async_trait]` marks the boxed futures it generates `#[must_use]`, which
// clippy's `double_must_use` flags on every trait method. Not actionable here.
#![allow(clippy::double_must_use)]
pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod shared;
