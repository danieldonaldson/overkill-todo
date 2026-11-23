//! Backend composition root: infrastructure adapters, wiring, and `APP_TARGETS`.
// `#[async_trait]` marks the boxed futures it generates `#[must_use]`, which
// clippy's `double_must_use` flags on every trait method. Not actionable here.
#![allow(
    clippy::double_must_use,
    reason = "`#[async_trait]` marks its boxed futures `#[must_use]`; not actionable here"
)]

pub mod infrastructure;

/// Tracing targets of every library and binary in the backend workspace.
///
/// Checked against `cargo metadata` by `tests/crate_graph.rs` so a new crate's
/// logs and spans can't silently go missing.
pub const APP_TARGETS: &[&str] = &[
    "shared",
    "domain",
    "application",
    "platform",
    "server",
    "overkill_todo",
];
