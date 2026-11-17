//! User role for authorization.

use std::fmt;

/// The role assigned to a user, determining their access level.
///
/// Roles form a hierarchy:
/// - `User`: Standard user with access to their own data
/// - `Admin`: Can manage users and view admin panels
/// - `SuperAdmin`: Full system access, can manage other admins
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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

    /// Whether this role may change an account's role from `current` to `new`
    /// (threat model P-08). The actor must be able to manage both the current
    /// and the new role, and a Super Admin can't remove their own Super Admin
    /// role (`acting_on_self`). Guarding the last Super Admin needs the other
    /// accounts, so the repository enforces it.
    ///
    /// # Errors
    /// The [`RoleChangeRefusal`] that applies; authority is checked first.
    pub const fn may_change_role(
        self,
        acting_on_self: bool,
        current: Self,
        new: Self,
    ) -> Result<(), RoleChangeRefusal> {
        if !self.can_manage(&current) || !self.can_manage(&new) {
            return Err(RoleChangeRefusal::NoAuthority);
        }
        if Self::removes_super_admin(current, new) && acting_on_self {
            return Err(RoleChangeRefusal::OwnSuperAdminRole);
        }
        Ok(())
    }

    /// Whether changing a role from `current` to `new` takes a Super Admin
    /// away: the change that can lock everyone out of role management.
    #[must_use]
    pub const fn removes_super_admin(current: Self, new: Self) -> bool {
        matches!(current, Self::SuperAdmin) && !matches!(new, Self::SuperAdmin)
    }
}

/// Why [`UserRole::may_change_role`] refuses a role change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleChangeRefusal {
    /// The actor can't manage the account's current role or the new one.
    NoAuthority,
    /// A Super Admin tried to remove their own Super Admin role.
    OwnSuperAdminRole,
}

/// Kani proofs (OT-010, threat model P-08). Run with `just verify`.
#[cfg(kani)]
mod verification {
    use super::*;

    /// Any of the three roles.
    fn any_role() -> UserRole {
        match kani::any::<u8>() {
            0 => UserRole::User,
            1 => UserRole::Admin,
            _ => UserRole::SuperAdmin,
        }
    }

    /// For every actor, current role, new role and self flag: a Member
    /// changes no role, an Admin only sets a Member to Member, and only a
    /// Super Admin touches an Admin or Super Admin, as the current role or the
    /// new one. Written out per role rather than through `can_manage`, so a
    /// change to `can_manage` fails here.
    #[kani::proof]
    fn role_changes_never_escalate() {
        let (actor, current, new) = (any_role(), any_role(), any_role());
        let allowed = actor.may_change_role(kani::any(), current, new).is_ok();
        match actor {
            UserRole::User => assert!(!allowed, "a Member changed a role"),
            UserRole::Admin => assert_eq!(
                allowed,
                current == UserRole::User && new == UserRole::User,
                "an Admin went beyond Member to Member"
            ),
            UserRole::SuperAdmin => {}
        }
        if allowed && (current != UserRole::User || new != UserRole::User) {
            assert_eq!(actor, UserRole::SuperAdmin);
        }
    }

    /// For every current and new role: a Super Admin may make any change to
    /// someone else, and never removes their own Super Admin role.
    #[kani::proof]
    fn a_super_admin_never_removes_their_own_role() {
        let (current, new) = (any_role(), any_role());
        let acting_on_self: bool = kani::any();
        let result = UserRole::SuperAdmin.may_change_role(acting_on_self, current, new);
        let removes = current == UserRole::SuperAdmin && new != UserRole::SuperAdmin;
        if acting_on_self && removes {
            assert_eq!(result, Err(RoleChangeRefusal::OwnSuperAdminRole));
        } else {
            assert_eq!(result, Ok(()));
        }
    }

    /// For every ASCII string of up to 12 bytes (the longest role name is 11):
    /// only the three exact role names are accepted, so an unknown stored role
    /// is rejected rather than defaulted, and each role reads back as itself.
    /// Non-ASCII strings are left out: they can't equal an ASCII name, and
    /// symbolic UTF-8 decoding multiplies the proof time.
    #[kani::proof]
    #[kani::unwind(13)]
    fn only_exact_role_names_are_accepted() {
        let bytes: [u8; 12] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        kani::assume(bytes.iter().all(u8::is_ascii));
        let Ok(s) = core::str::from_utf8(&bytes[..len]) else {
            unreachable!("ASCII is valid UTF-8");
        };
        match UserRole::from_db_str(s) {
            Some(role) => {
                assert!(matches!(s, "user" | "admin" | "super_admin"));
                assert_eq!(role.to_db_str(), s);
            }
            None => assert!(!matches!(s, "user" | "admin" | "super_admin")),
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
