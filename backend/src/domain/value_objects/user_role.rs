//! User role for authorization.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The role assigned to a user, determining their access level.
///
/// Roles form a hierarchy:
/// - `User`: Standard user with access to their own data
/// - `Admin`: Can manage users and view admin panels
/// - `SuperAdmin`: Full system access, can manage other admins
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, Default)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "user_role", rename_all = "snake_case")]
pub enum UserRole {
    /// Standard user - can only access their own data
    #[default]
    User,
    /// Admin - can manage users and access admin features
    Admin,
    /// Super admin - full system access, can manage other admins
    SuperAdmin,
}

impl UserRole {
    /// Returns all user role variants.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::User, Self::Admin, Self::SuperAdmin]
    }

    /// Converts from database string representation.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "admin" => Some(Self::Admin),
            "super_admin" => Some(Self::SuperAdmin),
            _ => None,
        }
    }

    /// Converts to database string representation.
    #[must_use]
    pub const fn to_db_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Admin => "admin",
            Self::SuperAdmin => "super_admin",
        }
    }

    /// Returns true if the user has admin privileges (Admin or `SuperAdmin`).
    #[must_use]
    pub const fn is_admin(&self) -> bool {
        matches!(self, Self::Admin | Self::SuperAdmin)
    }

    /// Returns true if the user is a super admin.
    #[must_use]
    pub const fn is_super_admin(&self) -> bool {
        matches!(self, Self::SuperAdmin)
    }

    /// Returns true if this role can manage the target role.
    ///
    /// - `SuperAdmin` can manage any role
    /// - Admin cannot manage other admins or super admins
    /// - User cannot manage anyone
    #[must_use]
    pub const fn can_manage(&self, target: &Self) -> bool {
        match (self, target) {
            // SuperAdmin can manage anyone
            // Admin can only manage regular users
            (Self::SuperAdmin, _) | (Self::Admin, Self::User) => true,
            // All other combinations are not allowed
            _ => false,
        }
    }
}

impl fmt::Display for UserRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User => write!(f, "User"),
            Self::Admin => write!(f, "Admin"),
            Self::SuperAdmin => write!(f, "Super Admin"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_user_role_db_roundtrip() {
        for role in UserRole::all() {
            let db_str = role.to_db_str();
            let parsed = UserRole::from_db_str(db_str);
            assert_eq!(parsed, Some(*role));
        }
    }

    #[test]
    fn test_user_role_default() {
        assert_eq!(UserRole::default(), UserRole::User);
    }

    #[test]
    fn test_user_role_is_admin() {
        assert!(!UserRole::User.is_admin());
        assert!(UserRole::Admin.is_admin());
        assert!(UserRole::SuperAdmin.is_admin());
    }

    #[test]
    fn test_user_role_is_super_admin() {
        assert!(!UserRole::User.is_super_admin());
        assert!(!UserRole::Admin.is_super_admin());
        assert!(UserRole::SuperAdmin.is_super_admin());
    }

    #[test]
    fn test_user_role_can_manage() {
        // SuperAdmin can manage anyone
        assert!(UserRole::SuperAdmin.can_manage(&UserRole::User));
        assert!(UserRole::SuperAdmin.can_manage(&UserRole::Admin));
        assert!(UserRole::SuperAdmin.can_manage(&UserRole::SuperAdmin));

        // Admin can only manage regular users
        assert!(UserRole::Admin.can_manage(&UserRole::User));
        assert!(!UserRole::Admin.can_manage(&UserRole::Admin));
        assert!(!UserRole::Admin.can_manage(&UserRole::SuperAdmin));

        // User cannot manage anyone
        assert!(!UserRole::User.can_manage(&UserRole::User));
        assert!(!UserRole::User.can_manage(&UserRole::Admin));
        assert!(!UserRole::User.can_manage(&UserRole::SuperAdmin));
    }

    #[test]
    fn test_user_role_from_invalid_db_str() {
        assert_eq!(UserRole::from_db_str("invalid"), None);
        assert_eq!(UserRole::from_db_str(""), None);
        assert_eq!(UserRole::from_db_str("USER"), None);
        assert_eq!(UserRole::from_db_str("superadmin"), None);
    }

    #[test]
    fn test_user_role_display() {
        assert_eq!(format!("{}", UserRole::User), "User");
        assert_eq!(format!("{}", UserRole::Admin), "Admin");
        assert_eq!(format!("{}", UserRole::SuperAdmin), "Super Admin");
    }

    // ==================== Property-Based Tests ====================

    /// Strategy for generating `UserRole` variants
    fn user_role_strategy() -> impl Strategy<Value = UserRole> {
        prop_oneof![
            Just(UserRole::User),
            Just(UserRole::Admin),
            Just(UserRole::SuperAdmin),
        ]
    }

    proptest! {
        // DB roundtrip: to_db_str -> from_db_str should return the original value
        #[test]
        fn prop_user_role_db_roundtrip(role in user_role_strategy()) {
            let db_str = role.to_db_str();
            let parsed = UserRole::from_db_str(db_str);
            prop_assert_eq!(parsed, Some(role));
        }

        // Invalid db strings should return None
        #[test]
        fn prop_invalid_db_str_returns_none(s in "[A-Z][a-z]{0,20}") {
            // Uppercase first letter ensures it won't match our lowercase db strings
            prop_assert!(UserRole::from_db_str(&s).is_none());
        }

        // Display output should be non-empty
        #[test]
        fn prop_display_non_empty(role in user_role_strategy()) {
            let display = format!("{role}");
            prop_assert!(!display.is_empty());
        }

        // All variants should be in all()
        #[test]
        fn prop_all_contains_every_variant(role in user_role_strategy()) {
            prop_assert!(UserRole::all().contains(&role));
        }

        // db_str should be lowercase (with underscores allowed)
        #[test]
        fn prop_db_str_lowercase(role in user_role_strategy()) {
            let db_str = role.to_db_str();
            prop_assert!(db_str.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }

        // SuperAdmin should always be able to manage any role
        #[test]
        fn prop_super_admin_manages_all(target in user_role_strategy()) {
            prop_assert!(UserRole::SuperAdmin.can_manage(&target));
        }

        // User should never be able to manage anyone
        #[test]
        fn prop_user_manages_none(target in user_role_strategy()) {
            prop_assert!(!UserRole::User.can_manage(&target));
        }

        // is_admin implies is_admin returns true for Admin and SuperAdmin
        #[test]
        fn prop_is_admin_consistency(role in user_role_strategy()) {
            let expected = matches!(role, UserRole::Admin | UserRole::SuperAdmin);
            prop_assert_eq!(role.is_admin(), expected);
        }

        // is_super_admin implies is_admin
        #[test]
        fn prop_super_admin_implies_admin(role in user_role_strategy()) {
            if role.is_super_admin() {
                prop_assert!(role.is_admin());
            }
        }
    }
}
