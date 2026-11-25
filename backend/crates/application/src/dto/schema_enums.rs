//! Schema-only enums for string fields with a fixed set of values.
//!
//! Some DTO fields stay `String` so their parsing and error behaviour is
//! unchanged (`SetUserRoleRequest.role` answers an unknown role with a validation
//! 400, not a deserialization error). These enums give those fields a precise
//! `OpenAPI` schema through `#[schema(value_type = …)]` without changing the Rust
//! type. They mirror domain enums, which must not depend on an API-doc crate;
//! the tests below fail if the two sets of values ever differ.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A user's role (domain `UserRole`).
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoleName {
    User,
    Admin,
    SuperAdmin,
}

/// A supported OAuth provider (domain `OAuthProvider`).
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OAuthProviderName {
    Google,
    Apple,
}

/// What kind of browser error a report describes (OT-006). Unlike the enums
/// above it is the field's real type: an unknown kind is rejected outright, so
/// it can't become a metric label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ClientErrorKind {
    /// An uncaught script error (`window.onerror`).
    Error,
    /// A promise rejection nobody handled.
    Unhandledrejection,
    /// An error caught by a panel's error boundary.
    Boundary,
    /// An API call that failed with a 5xx or never reached the server.
    Api,
}

impl ClientErrorKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Unhandledrejection => "unhandledrejection",
            Self::Boundary => "boundary",
            Self::Api => "api",
        }
    }
}

/// A Web Vital the browser measures (OT-006 Decision 6). Like
/// [`ClientErrorKind`] it is the field's real type, so an unknown name is
/// rejected and can't become a metric label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum VitalName {
    /// Largest Contentful Paint, ms, per full load.
    #[serde(rename = "LCP")]
    Lcp,
    /// First Contentful Paint, ms, per full load.
    #[serde(rename = "FCP")]
    Fcp,
    /// Time to First Byte, ms, per full load.
    #[serde(rename = "TTFB")]
    Ttfb,
    /// Interaction to Next Paint, ms, per page visit.
    #[serde(rename = "INP")]
    Inp,
    /// Cumulative Layout Shift, a unitless score, per page visit.
    #[serde(rename = "CLS")]
    Cls,
}

impl VitalName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lcp => "LCP",
            Self::Fcp => "FCP",
            Self::Ttfb => "TTFB",
            Self::Inp => "INP",
            Self::Cls => "CLS",
        }
    }

    /// `true` for the vitals measured in milliseconds; CLS is a score.
    #[must_use]
    pub const fn is_duration(self) -> bool {
        !matches!(self, Self::Cls)
    }
}

/// A Web Vital's rating against the standard thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Rating {
    Good,
    NeedsImprovement,
    Poor,
}

impl Rating {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::NeedsImprovement => "needs-improvement",
            Self::Poor => "poor",
        }
    }
}

/// How the browser reached a page (OT-006 Decision 6a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum NavType {
    /// A full page load, timed from the start of the document.
    Hard,
    /// An in-site navigation (`ClientRouter`), timed from its start.
    Soft,
}

impl NavType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hard => "hard",
            Self::Soft => "soft",
        }
    }
}

/// What happened to an in-site navigation's view transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ViewTransitionResult {
    /// The animation ran to the end.
    Ran,
    /// It was skipped: superseded by another navigation, or reduced motion.
    Skipped,
    /// The browser has no view transitions; the page was swapped without one.
    Unsupported,
    /// A full page load, which has no transition.
    None,
}

impl ViewTransitionResult {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ran => "ran",
            Self::Skipped => "skipped",
            Self::Unsupported => "unsupported",
            Self::None => "none",
        }
    }
}

/// Whether a navigation reached its page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum NavOutcome {
    Completed,
    /// Another navigation started before this one finished.
    Aborted,
}

impl NavOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Aborted => "aborted",
        }
    }
}

/// Which moment of a panel's loading an island timing marks (Decision 6b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum IslandStage {
    /// The island mounted and responds to input.
    Interactive,
    /// It shows its content, its empty state or its error state.
    Ready,
}

impl IslandStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "interactive",
            Self::Ready => "ready",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClientErrorKind, IslandStage, NavOutcome, NavType, OAuthProviderName, RoleName,
        ViewTransitionResult,
    };
    use domain::entities::OAuthProvider;
    use domain::value_objects::UserRole;

    fn wire<T: serde::Serialize>(value: T) -> String {
        serde_json::to_value(value)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn role_names_match_domain_roles() {
        for role in UserRole::all() {
            // Exhaustive: a new domain variant fails to compile until mapped here.
            let schema = match role {
                UserRole::User => RoleName::User,
                UserRole::Admin => RoleName::Admin,
                UserRole::SuperAdmin => RoleName::SuperAdmin,
            };
            assert_eq!(wire(schema), role.to_db_str());
        }
        assert_eq!(UserRole::all().len(), 3, "update RoleName and this count");
    }

    #[test]
    fn provider_names_match_domain_providers() {
        for provider in [OAuthProvider::Google, OAuthProvider::Apple] {
            // Exhaustive: a new domain variant fails to compile until mapped here.
            let schema = match provider {
                OAuthProvider::Google => OAuthProviderName::Google,
                OAuthProvider::Apple => OAuthProviderName::Apple,
            };
            assert_eq!(wire(schema), provider.as_str());
        }
    }

    #[test]
    fn client_error_kinds_serialize_as_their_label() {
        for kind in [
            ClientErrorKind::Error,
            ClientErrorKind::Unhandledrejection,
            ClientErrorKind::Boundary,
            ClientErrorKind::Api,
        ] {
            assert_eq!(wire(kind), kind.as_str());
        }
    }

    #[test]
    fn navigation_and_island_enums_serialize_as_their_label() {
        for v in [NavType::Hard, NavType::Soft] {
            assert_eq!(wire(v), v.as_str());
        }
        for v in [
            ViewTransitionResult::Ran,
            ViewTransitionResult::Skipped,
            ViewTransitionResult::Unsupported,
            ViewTransitionResult::None,
        ] {
            assert_eq!(wire(v), v.as_str());
        }
        for v in [NavOutcome::Completed, NavOutcome::Aborted] {
            assert_eq!(wire(v), v.as_str());
        }
        for v in [IslandStage::Interactive, IslandStage::Ready] {
            assert_eq!(wire(v), v.as_str());
        }
    }
}
