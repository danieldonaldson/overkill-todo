//! DTOs for the admin panel (`/api/v1/admin`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::domain::entities::User;

/// Default and maximum page sizes for the admin user listing.
pub const DEFAULT_USER_LIMIT: i64 = 50;
pub const MAX_USER_LIMIT: i64 = 200;

/// A user row in the admin user-management table.
#[derive(Debug, Serialize, ToSchema)]
pub struct AdminUserResponse {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub email_verified: bool,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_scheduled_at: Option<DateTime<Utc>>,
}

impl From<&User> for AdminUserResponse {
    fn from(user: &User) -> Self {
        Self {
            id: *user.id().as_uuid(),
            email: user.email().as_str().to_string(),
            role: user.role().to_db_str().to_string(),
            email_verified: user.email_verified(),
            created_at: user.created_at(),
            deletion_scheduled_at: user.deletion_scheduled_at(),
        }
    }
}

/// A page of users for the admin listing.
#[derive(Debug, Serialize, ToSchema)]
pub struct AdminUserListResponse {
    pub users: Vec<AdminUserResponse>,
    pub limit: i64,
    pub offset: i64,
}

/// Query parameters for the admin user listing.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListUsersQuery {
    /// Max users to return (defaults applied via [`Self::paginated`]).
    pub limit: Option<i64>,
    /// Offset for pagination.
    pub offset: Option<i64>,
}

impl ListUsersQuery {
    /// Resolve to `(limit, offset)`, applying defaults and clamping to sane
    /// bounds (limit in `[1, MAX_USER_LIMIT]`, offset `>= 0`).
    #[must_use]
    pub fn paginated(&self) -> (i64, i64) {
        let limit = self
            .limit
            .unwrap_or(DEFAULT_USER_LIMIT)
            .clamp(1, MAX_USER_LIMIT);
        let offset = self.offset.unwrap_or(0).max(0);
        (limit, offset)
    }
}

/// Request to change a user's role (super-admin only).
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetUserRoleRequest {
    /// New role: `user`, `admin`, or `super_admin`.
    pub role: String,
}
