//! Application layer — ports, DTOs, use cases.
#![allow(
    clippy::double_must_use,
    reason = "`#[async_trait]` marks its boxed futures `#[must_use]`; not actionable here"
)]

pub mod dto;
pub mod ports;
pub mod use_cases;
