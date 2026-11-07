use crate::domain::value_objects::{Email, UserId, UserRole};
use chrono::{DateTime, Utc};

/// A registered user in the system.
///
/// Users are the primary actors in the application.
///
/// # Password Handling
///
/// The `password_hash` field is optional to support OAuth-only users:
/// - Users who register via email/password will have `Some(hash)`
/// - Users who sign up via OAuth (Google/Apple) may have `None`
/// - Users can add a password later via password reset flow
///
/// # Email Change
///
/// Users can request to change their email. The flow is:
/// 1. User requests email change, `pending_email` is set
/// 2. Verification code is sent to the new email
/// 3. User confirms with the code, email is swapped
///
/// # Account Deletion
///
/// Users can request account deletion with a 14-day grace period:
/// 1. User requests deletion, `deletion_scheduled_at` is set to 14 days from now
/// 2. User can cancel deletion during the grace period
/// 3. After 14 days, a background job permanently deletes the account
///
/// # Examples
///
/// ```
/// use app::domain::{entities::User, value_objects::Email};
///
/// let email = Email::new("user@example.com").unwrap();
/// let user = User::new(
///     email,
///     Some("hashed_password".into()),
///     "John".into(),
///     "Doe".into(),
/// );
///
/// assert_eq!(user.first_name(), "John");
/// assert_eq!(user.last_name(), "Doe");
/// assert!(user.has_password());
/// ```
#[derive(Debug, Clone)]
pub struct User {
    id: UserId,
    email: Email,
    password_hash: Option<String>,
    first_name: String,
    last_name: String,
    email_verified: bool,
    pending_email: Option<Email>,
    pending_email_requested_at: Option<DateTime<Utc>>,
    deleted_at: Option<DateTime<Utc>>,
    deletion_scheduled_at: Option<DateTime<Utc>>,
    // Authorization
    role: UserRole,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// Optimistic-concurrency token. Carries the version this entity was loaded
    /// at; `save` applies its write only when the persisted row still matches,
    /// then bumps it. A fresh (unsaved) user starts at 0.
    version: i64,
}

/// The full set of a [`User`]'s fields in named form.
///
/// Used to cross the persistence and DTO boundaries
/// ([`User::from_persistence`] / [`User::into_parts`]) without a positional
/// tuple: because the fields are named, adding or reordering one is a compile
/// error at every mapping site rather than a silent positional mismatch.
pub struct UserParts {
    pub id: UserId,
    pub email: Email,
    pub password_hash: Option<String>,
    pub first_name: String,
    pub last_name: String,
    pub email_verified: bool,
    pub pending_email: Option<Email>,
    pub pending_email_requested_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub deletion_scheduled_at: Option<DateTime<Utc>>,
    pub role: UserRole,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Optimistic-concurrency version the row was loaded at (0 for a new user).
    pub version: i64,
}

impl User {
    /// Creates a new user with the given details.
    ///
    /// Automatically generates a unique [`UserId`] and sets `created_at`
    /// and `updated_at` to the current time.
    ///
    /// # Arguments
    ///
    /// * `email` - A validated [`Email`] address
    /// * `password_hash` - The pre-hashed password, or `None` for OAuth-only users
    /// * `first_name` - The user's first name
    /// * `last_name` - The user's last name
    ///
    /// # Examples
    ///
    /// ```
    /// use app::domain::{entities::User, value_objects::Email};
    ///
    /// // User with password
    /// let email = Email::new("jane@example.com").unwrap();
    /// let user = User::new(email, Some("hash".into()), "Jane".into(), "Smith".into());
    /// assert!(user.has_password());
    ///
    /// // OAuth-only user
    /// let email2 = Email::new("oauth@example.com").unwrap();
    /// let oauth_user = User::new(email2, None, "OAuth".into(), "User".into());
    /// assert!(!oauth_user.has_password());
    /// ```
    #[must_use]
    pub fn new(
        email: Email,
        password_hash: Option<String>,
        first_name: String,
        last_name: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: UserId::new(),
            email,
            password_hash,
            first_name,
            last_name,
            email_verified: false,
            pending_email: None,
            pending_email_requested_at: None,
            deleted_at: None,
            deletion_scheduled_at: None,
            role: UserRole::User,
            created_at: now,
            updated_at: now,
            version: 0,
        }
    }

    /// Reconstructs a user from persistence layer data.
    ///
    /// This is used by repositories when loading a user from the database.
    /// Unlike [`User::new`], this does not generate new IDs or timestamps.
    #[must_use]
    pub fn from_persistence(parts: UserParts) -> Self {
        let UserParts {
            id,
            email,
            password_hash,
            first_name,
            last_name,
            email_verified,
            pending_email,
            pending_email_requested_at,
            deleted_at,
            deletion_scheduled_at,
            role,
            created_at,
            updated_at,
            version,
        } = parts;
        Self {
            id,
            email,
            password_hash,
            first_name,
            last_name,
            email_verified,
            pending_email,
            pending_email_requested_at,
            deleted_at,
            deletion_scheduled_at,
            role,
            created_at,
            updated_at,
            version,
        }
    }

    /// Returns the user's unique identifier.
    #[must_use]
    pub const fn id(&self) -> &UserId {
        &self.id
    }

    /// Returns the user's email address.
    #[must_use]
    pub const fn email(&self) -> &Email {
        &self.email
    }

    /// Returns the user's hashed password, if set.
    ///
    /// Returns `None` for OAuth-only users who haven't set a password.
    /// Use Argon2 to verify passwords against this hash.
    #[must_use]
    pub fn password_hash(&self) -> Option<&str> {
        self.password_hash.as_deref()
    }

    /// Returns `true` if the user has a password set.
    ///
    /// OAuth-only users may not have a password until they set one
    /// via the password reset flow.
    #[must_use]
    pub const fn has_password(&self) -> bool {
        self.password_hash.is_some()
    }

    /// Returns the user's first name.
    #[must_use]
    pub fn first_name(&self) -> &str {
        &self.first_name
    }

    /// Returns the user's last name.
    #[must_use]
    pub fn last_name(&self) -> &str {
        &self.last_name
    }

    /// Returns when the user was created.
    #[must_use]
    pub const fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Returns when the user was last updated.
    #[must_use]
    pub const fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Returns the optimistic-concurrency version the entity was loaded at.
    #[must_use]
    pub const fn version(&self) -> i64 {
        self.version
    }

    /// Returns whether the user's email is verified.
    #[must_use]
    pub const fn email_verified(&self) -> bool {
        self.email_verified
    }

    /// Returns the pending email address, if any.
    #[must_use]
    pub const fn pending_email(&self) -> Option<&Email> {
        self.pending_email.as_ref()
    }

    /// Returns when the pending email was requested, if any.
    #[must_use]
    pub const fn pending_email_requested_at(&self) -> Option<DateTime<Utc>> {
        self.pending_email_requested_at
    }

    /// Returns true if there is a pending email change request.
    #[must_use]
    pub const fn has_pending_email(&self) -> bool {
        self.pending_email.is_some()
    }

    /// Returns the user's role.
    #[must_use]
    pub const fn role(&self) -> UserRole {
        self.role
    }

    /// Returns true if the user has admin privileges (Admin or `SuperAdmin`).
    #[must_use]
    pub const fn is_admin(&self) -> bool {
        self.role.is_admin()
    }

    /// Returns true if the user is a super admin.
    #[must_use]
    pub const fn is_super_admin(&self) -> bool {
        self.role.is_super_admin()
    }

    /// Updates the user's role.
    ///
    /// This also updates the `updated_at` timestamp.
    pub fn set_role(&mut self, role: UserRole) {
        self.role = role;
        self.updated_at = Utc::now();
    }

    /// Marks the user's email as verified.
    ///
    /// This also updates the `updated_at` timestamp.
    pub fn mark_email_verified(&mut self) {
        self.email_verified = true;
        self.updated_at = Utc::now();
    }

    /// Updates the user's password hash.
    ///
    /// This also updates the `updated_at` timestamp.
    /// Can be used to set a password for OAuth-only users.
    pub fn update_password(&mut self, password_hash: String) {
        self.password_hash = Some(password_hash);
        self.updated_at = Utc::now();
    }

    /// Requests an email change to a new address.
    ///
    /// This sets the `pending_email` and `pending_email_requested_at` fields.
    /// The email change must be confirmed by verifying the new email address.
    pub fn request_email_change(&mut self, new_email: Email) {
        let now = Utc::now();
        self.pending_email = Some(new_email);
        self.pending_email_requested_at = Some(now);
        self.updated_at = now;
    }

    /// Confirms the pending email change.
    ///
    /// This swaps the current email with the pending email and clears the pending fields.
    /// The `email_verified` flag is set to true since the new email was verified.
    pub fn confirm_email_change(&mut self) {
        if let Some(new_email) = self.pending_email.take() {
            self.email = new_email;
            self.pending_email_requested_at = None;
            self.email_verified = true;
            self.updated_at = Utc::now();
        }
    }

    /// Cancels any pending email change request.
    pub fn cancel_email_change(&mut self) {
        if self.pending_email.is_some() {
            self.pending_email = None;
            self.pending_email_requested_at = None;
            self.updated_at = Utc::now();
        }
    }

    /// Returns when the account was permanently deleted (soft-deleted), if any.
    #[must_use]
    pub const fn deleted_at(&self) -> Option<DateTime<Utc>> {
        self.deleted_at
    }

    /// Returns when the account is scheduled for deletion, if any.
    #[must_use]
    pub const fn deletion_scheduled_at(&self) -> Option<DateTime<Utc>> {
        self.deletion_scheduled_at
    }

    /// Returns true if the account is scheduled for deletion.
    #[must_use]
    pub const fn is_scheduled_for_deletion(&self) -> bool {
        self.deletion_scheduled_at.is_some()
    }

    /// Returns true if the account has been soft-deleted.
    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// Schedules the account for deletion after a grace period.
    ///
    /// The account will be permanently deleted 14 days from now unless cancelled.
    /// This sets `deletion_scheduled_at` but does not immediately delete the account.
    pub fn schedule_deletion(&mut self) {
        let now = Utc::now();
        // Schedule deletion for 14 days from now
        self.deletion_scheduled_at = Some(now + chrono::Duration::days(14));
        self.updated_at = now;
    }

    /// Cancels a pending account deletion.
    ///
    /// This clears the `deletion_scheduled_at` field, preventing the scheduled deletion.
    pub fn cancel_deletion(&mut self) {
        if self.deletion_scheduled_at.is_some() {
            self.deletion_scheduled_at = None;
            self.updated_at = Utc::now();
        }
    }

    /// Anonymizes the user's personal data for GDPR compliance.
    ///
    /// This replaces PII (email, names) with anonymized values while
    /// preserving the user record and its relationships with other data.
    /// The account is also marked as soft-deleted.
    ///
    /// # Arguments
    /// * `anonymous_email` - The anonymized email to use (e.g., `deleted-{uuid}@anonymized.local`)
    pub fn anonymize(&mut self, anonymous_email: Email) {
        let now = Utc::now();
        self.email = anonymous_email;
        self.password_hash = None;
        self.first_name = "Deleted".to_string();
        self.last_name = "User".to_string();
        self.pending_email = None;
        self.pending_email_requested_at = None;
        self.deleted_at = Some(now);
        self.deletion_scheduled_at = None;
        self.updated_at = now;
    }

    /// Returns true if the account deletion grace period has expired.
    ///
    /// This checks if `deletion_scheduled_at` is in the past.
    #[must_use]
    pub fn is_ready_for_deletion(&self) -> bool {
        self.deletion_scheduled_at
            .is_some_and(|scheduled| scheduled <= Utc::now())
    }

    /// Consumes the user and returns its fields as a named [`UserParts`].
    ///
    /// This is useful for converting to DTOs without cloning the owned fields.
    #[must_use]
    pub fn into_parts(self) -> UserParts {
        UserParts {
            id: self.id,
            email: self.email,
            password_hash: self.password_hash,
            first_name: self.first_name,
            last_name: self.last_name,
            email_verified: self.email_verified,
            pending_email: self.pending_email,
            pending_email_requested_at: self.pending_email_requested_at,
            deleted_at: self.deleted_at,
            deletion_scheduled_at: self.deletion_scheduled_at,
            role: self.role,
            created_at: self.created_at,
            updated_at: self.updated_at,
            version: self.version,
        }
    }

    /// Updates the user's first and last name.
    ///
    /// This also updates the `updated_at` timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// use app::domain::{entities::User, value_objects::Email};
    ///
    /// let email = Email::new("user@example.com").unwrap();
    /// let mut user = User::new(email, Some("hash".to_string()), "John".into(), "Doe".into());
    ///
    /// user.update_name("Jane".into(), "Smith".into());
    /// assert_eq!(user.first_name(), "Jane");
    /// assert_eq!(user.last_name(), "Smith");
    /// ```
    pub fn update_name(&mut self, first_name: String, last_name: String) {
        self.first_name = first_name;
        self.last_name = last_name;
        self.updated_at = Utc::now();
    }
}

/// Test-only builder for constructing a [`User`] in a specific state.
///
/// Without this, every test module that needs a fully-populated user (e.g. an
/// email-verified one) had to repeat the same 13-field [`UserParts`] literal,
/// most fields just to spell out the defaults. The builder defaults to a
/// verified email/password user and lets each test override only the fields it
/// cares about:
///
/// ```ignore
/// let user = UserBuilder::new().email("verify@example.com").email_verified(false).build();
/// ```
#[cfg(test)]
pub struct UserBuilder {
    parts: UserParts,
}

#[cfg(test)]
impl UserBuilder {
    /// Starts a builder for a verified email/password user with default fields.
    #[must_use]
    #[allow(clippy::missing_panics_doc)] // test-only; the default literal is valid
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            parts: UserParts {
                id: UserId::new(),
                email: Email::new("test@example.com").unwrap(),
                password_hash: Some("hashed_password".to_string()),
                first_name: "Test".to_string(),
                last_name: "User".to_string(),
                email_verified: true,
                pending_email: None,
                pending_email_requested_at: None,
                deleted_at: None,
                deletion_scheduled_at: None,
                role: UserRole::User,
                created_at: now,
                updated_at: now,
                version: 0,
            },
        }
    }

    /// Overrides the user id.
    #[must_use]
    pub fn id(mut self, id: UserId) -> Self {
        self.parts.id = id;
        self
    }

    /// Overrides the email address (must be a valid email).
    #[must_use]
    #[allow(clippy::missing_panics_doc)] // test-only; callers pass valid literals
    pub fn email(mut self, email: &str) -> Self {
        self.parts.email = Email::new(email).unwrap();
        self
    }

    /// Overrides the password hash; pass `None` for an OAuth-only user.
    #[must_use]
    pub fn password_hash(mut self, hash: Option<&str>) -> Self {
        self.parts.password_hash = hash.map(ToString::to_string);
        self
    }

    /// Overrides the first and last name.
    #[must_use]
    pub fn name(mut self, first: &str, last: &str) -> Self {
        self.parts.first_name = first.to_string();
        self.parts.last_name = last.to_string();
        self
    }

    /// Overrides whether the email is verified.
    #[must_use]
    pub fn email_verified(mut self, verified: bool) -> Self {
        self.parts.email_verified = verified;
        self
    }

    /// Overrides the soft-delete timestamp.
    #[must_use]
    pub fn deleted_at(mut self, deleted_at: Option<DateTime<Utc>>) -> Self {
        self.parts.deleted_at = deleted_at;
        self
    }

    /// Overrides the user's role.
    #[must_use]
    pub fn role(mut self, role: UserRole) -> Self {
        self.parts.role = role;
        self
    }

    /// Builds the [`User`].
    #[must_use]
    pub fn build(self) -> User {
        User::from_persistence(self.parts)
    }
}

#[cfg(test)]
impl Default for UserBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_email() -> Email {
        Email::new("test@example.com").unwrap()
    }

    #[test]
    fn test_new_user_with_password() {
        let user = User::new(
            valid_email(),
            Some("hashed_password".to_string()),
            "John".to_string(),
            "Doe".to_string(),
        );

        assert_eq!(user.email().as_str(), "test@example.com");
        assert_eq!(user.password_hash(), Some("hashed_password"));
        assert!(user.has_password());
        assert_eq!(user.first_name(), "John");
        assert_eq!(user.last_name(), "Doe");
        assert!(!user.email_verified());
    }

    #[test]
    fn test_new_user_without_password() {
        let user = User::new(valid_email(), None, "OAuth".to_string(), "User".to_string());

        assert_eq!(user.email().as_str(), "test@example.com");
        assert_eq!(user.password_hash(), None);
        assert!(!user.has_password());
        assert_eq!(user.first_name(), "OAuth");
        assert_eq!(user.last_name(), "User");
    }

    #[test]
    fn test_mark_email_verified() {
        let mut user = User::new(
            valid_email(),
            Some("hashed_password".to_string()),
            "John".to_string(),
            "Doe".to_string(),
        );

        assert!(!user.email_verified());
        let original_updated_at = user.updated_at();
        std::thread::sleep(std::time::Duration::from_millis(10));

        user.mark_email_verified();

        assert!(user.email_verified());
        assert!(user.updated_at() > original_updated_at);
    }

    #[test]
    fn test_update_password() {
        let mut user = User::new(
            valid_email(),
            Some("old_hash".to_string()),
            "John".to_string(),
            "Doe".to_string(),
        );

        let original_updated_at = user.updated_at();
        std::thread::sleep(std::time::Duration::from_millis(10));

        user.update_password("new_hash".to_string());

        assert_eq!(user.password_hash(), Some("new_hash"));
        assert!(user.updated_at() > original_updated_at);
    }

    #[test]
    fn test_set_password_for_oauth_user() {
        let mut user = User::new(valid_email(), None, "OAuth".to_string(), "User".to_string());

        assert!(!user.has_password());

        user.update_password("new_hash".to_string());

        assert!(user.has_password());
        assert_eq!(user.password_hash(), Some("new_hash"));
    }

    #[test]
    fn test_builder_defaults_and_overrides() {
        let default = UserBuilder::new().build();
        assert_eq!(default.email().as_str(), "test@example.com");
        assert!(default.has_password());
        assert!(default.email_verified());
        assert_eq!(default.role(), UserRole::User);

        let id = UserId::new();
        let now = Utc::now();
        let custom = UserBuilder::new()
            .id(id)
            .email("admin@example.com")
            .password_hash(None)
            .name("Ada", "Lovelace")
            .email_verified(false)
            .deleted_at(Some(now))
            .role(UserRole::Admin)
            .build();
        assert_eq!(custom.id(), &id);
        assert_eq!(custom.email().as_str(), "admin@example.com");
        assert!(!custom.has_password());
        assert_eq!(custom.first_name(), "Ada");
        assert_eq!(custom.last_name(), "Lovelace");
        assert!(!custom.email_verified());
        assert_eq!(custom.deleted_at(), Some(now));
        assert_eq!(custom.role(), UserRole::Admin);
    }

    #[test]
    fn test_update_name() {
        let mut user = User::new(
            valid_email(),
            Some("hashed_password".to_string()),
            "John".to_string(),
            "Doe".to_string(),
        );

        let original_updated_at = user.updated_at();
        std::thread::sleep(std::time::Duration::from_millis(10));

        user.update_name("Jane".to_string(), "Smith".to_string());

        assert_eq!(user.first_name(), "Jane");
        assert_eq!(user.last_name(), "Smith");
        assert!(user.updated_at() > original_updated_at);
    }

    #[test]
    fn test_set_role_updates_role_and_timestamp() {
        let mut user = UserBuilder::new().build();
        assert!(!user.is_admin());
        let original = user.updated_at();
        std::thread::sleep(std::time::Duration::from_millis(10));

        user.set_role(UserRole::SuperAdmin);

        assert_eq!(user.role(), UserRole::SuperAdmin);
        assert!(user.is_admin());
        assert!(user.is_super_admin());
        assert!(user.updated_at() > original);
    }

    #[test]
    fn test_admin_role_is_admin_but_not_super_admin() {
        let user = UserBuilder::new().role(UserRole::Admin).build();
        assert!(user.is_admin());
        assert!(!user.is_super_admin());
    }

    #[test]
    fn test_request_email_change_sets_pending_fields() {
        let mut user = UserBuilder::new().build();
        assert!(!user.has_pending_email());
        assert!(user.pending_email().is_none());
        assert!(user.pending_email_requested_at().is_none());

        let new_email = Email::new("new@example.com").unwrap();
        user.request_email_change(new_email.clone());

        assert!(user.has_pending_email());
        assert_eq!(user.pending_email(), Some(&new_email));
        assert!(user.pending_email_requested_at().is_some());
    }

    #[test]
    fn test_confirm_email_change_swaps_email() {
        let mut user = UserBuilder::new().email_verified(false).build();
        let new_email = Email::new("new@example.com").unwrap();
        user.request_email_change(new_email.clone());

        user.confirm_email_change();

        assert_eq!(user.email(), &new_email);
        assert!(user.email_verified());
        assert!(!user.has_pending_email());
        assert!(user.pending_email_requested_at().is_none());
    }

    #[test]
    fn test_confirm_email_change_without_pending_is_noop() {
        let mut user = UserBuilder::new().build();
        let before = user.email().clone();
        let updated_before = user.updated_at();

        user.confirm_email_change();

        assert_eq!(user.email(), &before);
        assert_eq!(user.updated_at(), updated_before);
    }

    #[test]
    fn test_cancel_email_change_clears_pending() {
        let mut user = UserBuilder::new().build();
        user.request_email_change(Email::new("new@example.com").unwrap());
        assert!(user.has_pending_email());

        user.cancel_email_change();

        assert!(!user.has_pending_email());
        assert!(user.pending_email_requested_at().is_none());
    }

    #[test]
    fn test_cancel_email_change_without_pending_is_noop() {
        let mut user = UserBuilder::new().build();
        let updated_before = user.updated_at();

        user.cancel_email_change();

        assert!(!user.has_pending_email());
        assert_eq!(user.updated_at(), updated_before);
    }

    #[test]
    fn test_schedule_deletion_sets_future_timestamp() {
        let mut user = UserBuilder::new().build();
        assert!(!user.is_scheduled_for_deletion());

        user.schedule_deletion();

        assert!(user.is_scheduled_for_deletion());
        let scheduled = user.deletion_scheduled_at().unwrap();
        assert!(scheduled > Utc::now());
        // Grace period is 14 days; not ready yet.
        assert!(!user.is_ready_for_deletion());
    }

    #[test]
    fn test_cancel_deletion_clears_schedule() {
        let mut user = UserBuilder::new().build();
        user.schedule_deletion();
        assert!(user.is_scheduled_for_deletion());

        user.cancel_deletion();

        assert!(!user.is_scheduled_for_deletion());
        assert!(user.deletion_scheduled_at().is_none());
    }

    #[test]
    fn test_cancel_deletion_without_schedule_is_noop() {
        let mut user = UserBuilder::new().build();
        let updated_before = user.updated_at();

        user.cancel_deletion();

        assert!(!user.is_scheduled_for_deletion());
        assert_eq!(user.updated_at(), updated_before);
    }

    #[test]
    fn test_is_ready_for_deletion_when_scheduled_in_past() {
        let parts = UserBuilder::new().build().into_parts();
        let user = User::from_persistence(UserParts {
            deletion_scheduled_at: Some(Utc::now() - chrono::Duration::days(1)),
            ..parts
        });
        assert!(user.is_ready_for_deletion());
    }

    #[test]
    fn test_is_ready_for_deletion_false_when_not_scheduled() {
        let user = UserBuilder::new().build();
        assert!(!user.is_ready_for_deletion());
    }

    #[test]
    fn test_anonymize_replaces_pii_and_soft_deletes() {
        let mut user = UserBuilder::new().build();
        user.request_email_change(Email::new("new@example.com").unwrap());
        user.schedule_deletion();
        assert!(!user.is_deleted());

        let anon = Email::new("deleted-123@anonymized.local").unwrap();
        user.anonymize(anon.clone());

        assert_eq!(user.email(), &anon);
        assert!(!user.has_password());
        assert_eq!(user.first_name(), "Deleted");
        assert_eq!(user.last_name(), "User");
        assert!(!user.has_pending_email());
        assert!(user.pending_email_requested_at().is_none());
        assert!(user.is_deleted());
        assert!(user.deleted_at().is_some());
        assert!(!user.is_scheduled_for_deletion());
    }

    #[test]
    fn test_into_parts_and_from_persistence_roundtrip() {
        let user = UserBuilder::new()
            .email("round@example.com")
            .role(UserRole::Admin)
            .build();
        let id = *user.id();
        let created = user.created_at();
        let version = user.version();

        let parts = user.into_parts();
        let rebuilt = User::from_persistence(parts);

        assert_eq!(rebuilt.id(), &id);
        assert_eq!(rebuilt.email().as_str(), "round@example.com");
        assert_eq!(rebuilt.role(), UserRole::Admin);
        assert_eq!(rebuilt.created_at(), created);
        assert_eq!(rebuilt.version(), version);
    }

    #[test]
    fn test_new_user_defaults() {
        let user = User::new(valid_email(), None, "A".into(), "B".into());
        assert_eq!(user.version(), 0);
        assert_eq!(user.role(), UserRole::User);
        assert!(!user.is_admin());
        assert!(!user.is_deleted());
        assert!(!user.is_scheduled_for_deletion());
        assert_eq!(user.created_at(), user.updated_at());
    }
}
