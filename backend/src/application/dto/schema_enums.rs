//! Schema-only enums for string fields with a fixed set of values.
//!
//! Some DTO fields stay `String` so their parsing and error behaviour is
//! unchanged (`SetUserRoleRequest.role` answers an unknown role with a validation
//! 400, not a deserialization error). These enums give those fields a precise
//! `OpenAPI` schema through `#[schema(value_type = …)]` without changing the Rust
//! type. They mirror domain enums, which must not depend on an API-doc crate;
//! the tests below fail if the two sets of values ever differ.

use serde::Serialize;
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

#[cfg(test)]
mod tests {
    use super::{OAuthProviderName, RoleName};
    use crate::domain::entities::OAuthProvider;
    use crate::domain::value_objects::UserRole;

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
}
