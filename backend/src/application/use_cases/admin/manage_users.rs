//! Admin use cases for listing users and managing roles.

use std::sync::Arc;

use crate::application::ports::outgoing::UserRepository;
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::{UserId, UserRole};

/// Lists users (paginated) for the admin user-management table.
pub struct ListUsersUseCase {
    users: Arc<dyn UserRepository>,
}

impl ListUsersUseCase {
    #[must_use]
    pub const fn new(users: Arc<dyn UserRepository>) -> Self {
        Self { users }
    }

    /// # Errors
    /// Propagates persistence failures.
    #[tracing::instrument(name = "usecase.admin.list_users", skip(self))]
    pub async fn execute(&self, limit: i64, offset: i64) -> Result<Vec<User>, DomainError> {
        self.users.find_all(limit, offset).await
    }
}

/// Changes a user's role, enforcing the actor's management authority.
pub struct SetUserRoleUseCase {
    users: Arc<dyn UserRepository>,
}

impl SetUserRoleUseCase {
    #[must_use]
    pub const fn new(users: Arc<dyn UserRepository>) -> Self {
        Self { users }
    }

    /// Sets `target`'s role to `new_role`. The `actor_role` must be able to
    /// manage both the target's current role and the new role.
    ///
    /// Two guards protect against locking everyone out of role management:
    /// an actor cannot strip their own super-admin role (self-lockout), and the
    /// last remaining super-admin cannot be demoted by anyone.
    ///
    /// # Errors
    /// - [`DomainError::Validation`] if `new_role` is not a valid role string.
    /// - [`DomainError::Forbidden`] if the actor can't manage the target/new
    ///   role, is demoting themselves out of super-admin, or is demoting the
    ///   last super-admin.
    /// - [`DomainError::NotFound`] if the user does not exist.
    /// - Persistence / [`DomainError::Conflict`] from the repository.
    #[tracing::instrument(name = "usecase.admin.set_user_role", skip(self), fields(actor = %actor_id, target = %target, new_role))]
    pub async fn execute(
        &self,
        actor_role: UserRole,
        actor_id: &UserId,
        target: &UserId,
        new_role: &str,
    ) -> Result<User, DomainError> {
        let new_role = UserRole::from_db_str(new_role)
            .ok_or_else(|| DomainError::Validation(format!("Invalid role: {new_role}")))?;

        let mut user = self
            .users
            .find_by_id(target)
            .await?
            .ok_or_else(|| DomainError::NotFound("User not found".into()))?;

        // The actor must outrank both the current and the requested role, so an
        // admin can't (e.g.) demote a super_admin or promote anyone to admin.
        if !actor_role.can_manage(&user.role()) || !actor_role.can_manage(&new_role) {
            return Err(DomainError::Forbidden(
                "You do not have authority to set this role".into(),
            ));
        }

        // Demoting a super-admin is the only role change that can brick role
        // management, so it carries two extra guards.
        let removing_super_admin =
            user.role() == UserRole::SuperAdmin && new_role != UserRole::SuperAdmin;
        if removing_super_admin {
            // Self-lockout guard: a super-admin can't strip their own role; a
            // second super-admin must do it for them.
            if actor_id == target {
                return Err(DomainError::Forbidden(
                    "You cannot remove your own super-admin role".into(),
                ));
            }
        }

        let role_changed = user.role() != new_role;
        user.set_role(new_role);
        if role_changed {
            self.users
                .save_role_change(&user, removing_super_admin)
                .await?;
        } else {
            self.users.save(&user).await?;
        }
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::UserBuilder;

    #[tokio::test]
    async fn set_user_role_rejects_invalid_role() {
        let users = MockUserRepository::new();
        let uc = SetUserRoleUseCase::new(Arc::new(users));
        let err = uc
            .execute(
                UserRole::SuperAdmin,
                &UserId::new(),
                &UserId::new(),
                "wizard",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[tokio::test]
    async fn set_user_role_forbids_admin_managing_admin() {
        let mut users = MockUserRepository::new();
        users
            .expect_find_by_id()
            .returning(|_| Ok(Some(UserBuilder::new().role(UserRole::Admin).build())));
        users.expect_save().never();

        let uc = SetUserRoleUseCase::new(Arc::new(users));
        let err = uc
            .execute(UserRole::Admin, &UserId::new(), &UserId::new(), "user")
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Forbidden(_)));
    }

    #[tokio::test]
    async fn set_user_role_super_admin_promotes() {
        let mut users = MockUserRepository::new();
        users
            .expect_find_by_id()
            .returning(|_| Ok(Some(UserBuilder::new().role(UserRole::User).build())));
        users
            .expect_save_role_change()
            .withf(|u, removing| u.role() == UserRole::Admin && !removing)
            .times(1)
            .returning(|_, _| Ok(()));

        let uc = SetUserRoleUseCase::new(Arc::new(users));
        let user = uc
            .execute(
                UserRole::SuperAdmin,
                &UserId::new(),
                &UserId::new(),
                "admin",
            )
            .await
            .unwrap();
        assert_eq!(user.role(), UserRole::Admin);
    }

    #[tokio::test]
    async fn set_user_role_forbids_self_demotion() {
        let id = UserId::new();
        let mut users = MockUserRepository::new();
        users.expect_find_by_id().returning(move |_| {
            Ok(Some(
                UserBuilder::new().id(id).role(UserRole::SuperAdmin).build(),
            ))
        });
        // Self-demotion is refused before any super-admin count is consulted.
        users.expect_save().never();
        users.expect_save_role_change().never();

        let uc = SetUserRoleUseCase::new(Arc::new(users));
        let err = uc
            .execute(UserRole::SuperAdmin, &id, &id, "user")
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Forbidden(_)));
    }

    #[tokio::test]
    async fn set_user_role_forbids_demoting_last_super_admin() {
        let mut users = MockUserRepository::new();
        users
            .expect_find_by_id()
            .returning(|_| Ok(Some(UserBuilder::new().role(UserRole::SuperAdmin).build())));
        users.expect_save_role_change().returning(|_, removing| {
            assert!(removing);
            Err(DomainError::Forbidden(
                "Cannot demote the last super-admin".into(),
            ))
        });

        let uc = SetUserRoleUseCase::new(Arc::new(users));
        let err = uc
            .execute(UserRole::SuperAdmin, &UserId::new(), &UserId::new(), "user")
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Forbidden(_)));
    }

    #[tokio::test]
    async fn set_user_role_demotes_super_admin_when_another_remains() {
        let mut users = MockUserRepository::new();
        users
            .expect_find_by_id()
            .returning(|_| Ok(Some(UserBuilder::new().role(UserRole::SuperAdmin).build())));
        users
            .expect_save_role_change()
            .withf(|u, removing| u.role() == UserRole::User && *removing)
            .times(1)
            .returning(|_, removing| {
                assert!(removing);
                Ok(())
            });

        let uc = SetUserRoleUseCase::new(Arc::new(users));
        let user = uc
            .execute(UserRole::SuperAdmin, &UserId::new(), &UserId::new(), "user")
            .await
            .unwrap();
        assert_eq!(user.role(), UserRole::User);
    }
}
