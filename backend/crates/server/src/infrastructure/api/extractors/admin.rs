//! Admin authorization extractors.
//!
//! These extractors enforce role-based access control for admin endpoints.
//! Use `AdminUser` for endpoints that require admin or `super_admin` role.
//! Use `SuperAdminUser` for endpoints that require `super_admin` role only.

use axum::{extract::FromRequestParts, http::request::Parts};

use crate::{
    domain::value_objects::{UserId, UserRole},
    infrastructure::api::error::AppError,
    infrastructure::api::middleware::AuthUser,
};

/// Extractor that requires admin or `super_admin` role.
///
/// Use this for admin endpoints that both admins and `super_admins` can access.
///
/// # Example
///
/// ```ignore
/// async fn list_users(admin: AdminUser) -> Result<Json<Vec<User>>, AppError> {
///     // Only reachable if user has admin or `super_admin` role
/// }
/// ```
#[derive(Clone, Debug)]
pub struct AdminUser {
    pub user_id: UserId,
    pub email_verified: bool,
    pub role: UserRole,
}

impl AdminUser {
    /// Returns true if the user is a super admin.
    #[must_use]
    pub const fn is_super_admin(&self) -> bool {
        self.role.is_super_admin()
    }
}

impl<S> FromRequestParts<S> for AdminUser
where
    S: Send + Sync,
{
    type Rejection = AppError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        // Nothing to await: resolve synchronously and hand back a ready future.
        let result = (|| -> Result<Self, Self::Rejection> {
            let auth_user = parts
                .extensions
                .get::<AuthUser>()
                .cloned()
                .ok_or_else(AppError::token_missing)?;

            if !auth_user.role.is_admin() {
                return Err(AppError::access_denied());
            }

            Ok(Self {
                user_id: auth_user.user_id,
                email_verified: auth_user.email_verified,
                role: auth_user.role,
            })
        })();
        std::future::ready(result)
    }
}

/// Extractor that requires `super_admin` role only.
///
/// Use this for sensitive admin operations like promoting users to admin.
///
/// # Example
///
/// ```ignore
/// async fn promote_to_admin(super_admin: SuperAdminUser, user_id: Path<Uuid>) -> Result<(), AppError> {
///     // Only reachable if user has `super_admin` role
/// }
/// ```
#[derive(Clone, Debug)]
pub struct SuperAdminUser {
    pub user_id: UserId,
    pub email_verified: bool,
}

impl<S> FromRequestParts<S> for SuperAdminUser
where
    S: Send + Sync,
{
    type Rejection = AppError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        // Nothing to await: resolve synchronously and hand back a ready future.
        let result = (|| -> Result<Self, Self::Rejection> {
            let auth_user = parts
                .extensions
                .get::<AuthUser>()
                .cloned()
                .ok_or_else(AppError::token_missing)?;

            if !auth_user.role.is_super_admin() {
                return Err(AppError::access_denied());
            }

            Ok(Self {
                user_id: auth_user.user_id,
                email_verified: auth_user.email_verified,
            })
        })();
        std::future::ready(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_admin_user_is_super_admin() {
        let admin = AdminUser {
            user_id: UserId::new(),
            email_verified: true,
            role: UserRole::Admin,
        };
        assert!(!admin.is_super_admin());

        let super_admin = AdminUser {
            user_id: UserId::new(),
            email_verified: true,
            role: UserRole::SuperAdmin,
        };
        assert!(super_admin.is_super_admin());
    }

    #[test]
    fn test_admin_user_clone() {
        let admin = AdminUser {
            user_id: UserId::new(),
            email_verified: true,
            role: UserRole::Admin,
        };

        let cloned = admin.clone();
        assert_eq!(cloned.user_id, admin.user_id);
        assert_eq!(cloned.email_verified, admin.email_verified);
        assert_eq!(cloned.role, admin.role);
    }

    #[test]
    fn test_super_admin_user_clone() {
        let super_admin = SuperAdminUser {
            user_id: UserId::new(),
            email_verified: true,
        };

        let cloned = super_admin.clone();
        assert_eq!(cloned.user_id, super_admin.user_id);
        assert_eq!(cloned.email_verified, super_admin.email_verified);
    }

    #[test]
    fn test_admin_user_debug() {
        let admin = AdminUser {
            user_id: UserId::new(),
            email_verified: true,
            role: UserRole::Admin,
        };

        let debug_str = format!("{admin:?}");
        assert!(debug_str.contains("AdminUser"));
        assert!(debug_str.contains("role"));
    }

    #[test]
    fn test_super_admin_user_debug() {
        let super_admin = SuperAdminUser {
            user_id: UserId::new(),
            email_verified: true,
        };

        let debug_str = format!("{super_admin:?}");
        assert!(debug_str.contains("SuperAdminUser"));
    }

    // ----- from_request_parts rejection / acceptance paths -----

    use crate::infrastructure::api::error::{auth_codes, forbidden_codes};

    /// Build a bare `Parts`, optionally seeding an `AuthUser` extension with the
    /// given role, mirroring what `auth_middleware` injects upstream.
    fn parts_with_role(role: Option<UserRole>) -> axum::http::request::Parts {
        let (mut parts, ()) = axum::http::Request::builder()
            .body(())
            .unwrap()
            .into_parts();
        if let Some(role) = role {
            parts.extensions.insert(AuthUser {
                user_id: UserId::new(),
                email_verified: true,
                role,
                issued_at: 0,
            });
        }
        parts
    }

    // --- AdminUser ---

    #[tokio::test]
    async fn admin_user_without_auth_extension_is_token_missing() {
        let mut parts = parts_with_role(None);
        let err = AdminUser::from_request_parts(&mut parts, &())
            .await
            .expect_err("missing AuthUser must reject");
        match err {
            AppError::Authorization { code, .. } => {
                assert_eq!(code, auth_codes::TOKEN_MISSING);
            }
            other => panic!("expected token_missing Authorization error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn admin_user_with_plain_user_role_is_access_denied() {
        let mut parts = parts_with_role(Some(UserRole::User));
        let err = AdminUser::from_request_parts(&mut parts, &())
            .await
            .expect_err("non-admin must be rejected");
        match err {
            AppError::Forbidden { code, .. } => {
                assert_eq!(code, forbidden_codes::ACCESS_DENIED);
            }
            other => panic!("expected access_denied Forbidden error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn admin_user_with_admin_role_succeeds() {
        let mut parts = parts_with_role(Some(UserRole::Admin));
        let admin = AdminUser::from_request_parts(&mut parts, &())
            .await
            .expect("admin role must be accepted");
        assert_eq!(admin.role, UserRole::Admin);
        assert!(!admin.is_super_admin());
    }

    #[tokio::test]
    async fn admin_user_with_super_admin_role_succeeds() {
        let mut parts = parts_with_role(Some(UserRole::SuperAdmin));
        let admin = AdminUser::from_request_parts(&mut parts, &())
            .await
            .expect("super_admin role must be accepted by AdminUser");
        assert!(admin.is_super_admin());
    }

    // --- SuperAdminUser ---

    #[tokio::test]
    async fn super_admin_user_without_auth_extension_is_token_missing() {
        let mut parts = parts_with_role(None);
        let err = SuperAdminUser::from_request_parts(&mut parts, &())
            .await
            .expect_err("missing AuthUser must reject");
        match err {
            AppError::Authorization { code, .. } => {
                assert_eq!(code, auth_codes::TOKEN_MISSING);
            }
            other => panic!("expected token_missing Authorization error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn super_admin_user_with_admin_role_is_access_denied() {
        // A mere admin (not super_admin) must be rejected by SuperAdminUser.
        let mut parts = parts_with_role(Some(UserRole::Admin));
        let err = SuperAdminUser::from_request_parts(&mut parts, &())
            .await
            .expect_err("admin (non-super) must be rejected");
        match err {
            AppError::Forbidden { code, .. } => {
                assert_eq!(code, forbidden_codes::ACCESS_DENIED);
            }
            other => panic!("expected access_denied Forbidden error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn super_admin_user_with_plain_user_role_is_access_denied() {
        let mut parts = parts_with_role(Some(UserRole::User));
        let err = SuperAdminUser::from_request_parts(&mut parts, &())
            .await
            .expect_err("non-admin must be rejected");
        assert!(matches!(err, AppError::Forbidden { .. }));
    }

    #[tokio::test]
    async fn super_admin_user_with_super_admin_role_succeeds() {
        let mut parts = parts_with_role(Some(UserRole::SuperAdmin));
        let super_admin = SuperAdminUser::from_request_parts(&mut parts, &())
            .await
            .expect("super_admin role must be accepted");
        assert!(super_admin.email_verified);
    }
}
