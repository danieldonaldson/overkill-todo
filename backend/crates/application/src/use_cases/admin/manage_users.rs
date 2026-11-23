//! Admin use cases for listing users and managing roles.

use std::sync::Arc;

use crate::application::ports::outgoing::UserRepository;
use crate::domain::entities::{Actor, AuditEvent, User};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::{RoleChangeRefusal, UserId, UserRole};

/// One page of users plus the total across all pages.
#[derive(Debug)]
pub struct UserPage {
    pub users: Vec<User>,
    pub total: i64,
}

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
    #[tracing::instrument(name = "usecase.admin.list_users", skip_all, fields(limit = limit, offset = offset))]
    pub async fn execute(&self, limit: i64, offset: i64) -> Result<UserPage, DomainError> {
        // Independent reads: fetch the page and the total concurrently.
        let (users, total) =
            tokio::try_join!(self.users.find_all(limit, offset), self.users.count_all(),)?;
        Ok(UserPage { users, total })
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
    #[tracing::instrument(name = "usecase.admin.set_user_role", skip_all, fields(actor = %actor_id, target = %target, new_role))]
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

        // The actor must outrank both the current and the requested role, and a
        // super-admin can't strip their own role (proven in the domain, P-08).
        actor_role
            .may_change_role(actor_id == target, user.role(), new_role)
            .map_err(|refusal| {
                DomainError::Forbidden(
                    match refusal {
                        RoleChangeRefusal::NoAuthority => {
                            "You do not have authority to set this role"
                        }
                        RoleChangeRefusal::OwnSuperAdminRole => {
                            "You cannot remove your own super-admin role"
                        }
                    }
                    .into(),
                )
            })?;

        // Demoting the last super-admin would brick role management; the
        // repository refuses it in the same statement as the write.
        let removing_super_admin = UserRole::removes_super_admin(user.role(), new_role);

        let old_role = user.role();
        user.set_role(new_role);
        if old_role == new_role {
            // Same role: nothing changes, so nothing is audited.
            self.users.save(&user).await?;
        } else {
            // The repository stores the event in the role change's own
            // transaction, so a refusal or failure there leaves no event.
            let event =
                AuditEvent::role_changed(*target, Actor::User(*actor_id), old_role, new_role);
            self.users
                .save_role_change(&user, removing_super_admin, &event)
                .await?;
        }
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::{AuditDetails, UserBuilder};

    #[tokio::test]
    async fn set_user_role_to_the_same_role_is_not_audited() {
        let mut users = MockUserRepository::new();
        users
            .expect_find_by_id()
            .returning(|_| Ok(Some(UserBuilder::new().role(UserRole::Admin).build())));
        users.expect_save_role_change().never();
        users.expect_save().times(1).returning(|_| Ok(()));

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
    async fn set_user_role_missing_user_is_not_audited() {
        let mut users = MockUserRepository::new();
        users.expect_find_by_id().returning(|_| Ok(None));
        users.expect_save_role_change().never();
        users.expect_save().never();

        let uc = SetUserRoleUseCase::new(Arc::new(users));
        let err = uc
            .execute(
                UserRole::SuperAdmin,
                &UserId::new(),
                &UserId::new(),
                "admin",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)));
    }

    #[tokio::test]
    async fn list_users_returns_page_and_total() {
        let mut users = MockUserRepository::new();
        users
            .expect_find_all()
            .withf(|limit, offset| *limit == 50 && *offset == 100)
            .returning(|_, _| Ok(Vec::new()));
        users.expect_count_all().returning(|| Ok(500));
        let page = ListUsersUseCase::new(Arc::new(users))
            .execute(50, 100)
            .await
            .unwrap();
        assert!(page.users.is_empty());
        assert_eq!(page.total, 500);
    }

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
        users.expect_save_role_change().never();

        let uc = SetUserRoleUseCase::new(Arc::new(users));
        let err = uc
            .execute(UserRole::Admin, &UserId::new(), &UserId::new(), "user")
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Forbidden(_)));
    }

    #[tokio::test]
    async fn set_user_role_super_admin_promotes() {
        let (actor, target) = (UserId::new(), UserId::new());
        let mut users = MockUserRepository::new();
        users
            .expect_find_by_id()
            .returning(|_| Ok(Some(UserBuilder::new().role(UserRole::User).build())));
        users
            .expect_save_role_change()
            .withf(move |u, removing, event| {
                u.role() == UserRole::Admin
                    && !removing
                    && *event
                        == AuditEvent::role_changed(
                            target,
                            Actor::User(actor),
                            UserRole::User,
                            UserRole::Admin,
                        )
            })
            .times(1)
            .returning(|_, _, _| Ok(()));

        let uc = SetUserRoleUseCase::new(Arc::new(users));
        let user = uc
            .execute(UserRole::SuperAdmin, &actor, &target, "admin")
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
        users.expect_save_role_change().returning(|_, removing, _| {
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
            .withf(|u, removing, event| {
                u.role() == UserRole::User
                    && *removing
                    && event.details()
                        == AuditDetails::RoleChanged {
                            from: UserRole::SuperAdmin,
                            to: UserRole::User,
                        }
            })
            .times(1)
            .returning(|_, removing, _| {
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
