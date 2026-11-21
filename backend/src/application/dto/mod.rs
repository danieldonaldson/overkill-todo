//! Application DTOs for the auth slice.

pub mod admin_dto;
pub mod audit_dto;

pub mod auth_dto;

pub mod client_telemetry_dto;

pub mod oauth_dto;
pub mod profile_dto;
pub mod schema_enums;
pub mod validation;

pub use admin_dto::*;
pub use audit_dto::*;

pub use auth_dto::*;

pub use client_telemetry_dto::{
    ClientErrorEvent, ClientEvent, ClientIslandEvent, ClientNavigationEvent, ClientReportBatch,
    ClientVitalEvent,
};

pub use oauth_dto::*;
pub use profile_dto::*;
pub use schema_enums::{
    ClientErrorKind, IslandStage, NavOutcome, NavType, OAuthProviderName, Rating, RoleName,
    ViewTransitionResult, VitalName,
};
