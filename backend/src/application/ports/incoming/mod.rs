//! Incoming ports.
//!
//! Most use cases are concrete structs that their handlers call directly.
//! [`RecordClientTelemetry`] is a trait so the browser-report endpoint depends
//! only on the operation and could move to its own service (OT-006 Decision 2).

pub mod record_client_telemetry;

pub use record_client_telemetry::RecordClientTelemetry;
