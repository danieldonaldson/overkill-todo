## Purpose

Keeps a permanent, unchangeable record of privileged and security-relevant
account actions (who did what to which account, and when), with minimal
personal data, and defines who may read it.

## ADDED Requirements

### Requirement: Account actions are recorded as audit events
The system SHALL record an audit event for each of these actions:

- role changed: by a Super Admin in the app, or by the operator's admin bootstrap command
- password changed, password set (first password on an account without one), password reset with a reset code
- sign-in method linked or unlinked (for example Google)
- account deletion requested, deletion cancelled, account deleted at the end of the grace period
- account data exported
- all sessions signed out because a revoked session token was reused

Each event SHALL hold the time, the kind of event, the affected account, who
acted (an account, "system" for automatic actions, or "operator" for the
bootstrap command), the trace ID of the request or job that caused it, and
only these details: for a role change, the old and new role; for a sign-in
method, the provider.

#### Scenario: Super Admin changes a role
- **WHEN** a Super Admin changes a Member's role to Admin
- **THEN** an event records the time, "role changed", the Member's account, the Super Admin's account as the actor, old role Member and new role Admin

#### Scenario: Member changes their password
- **WHEN** a Member changes their password
- **THEN** an event records "password changed" with the Member as both the affected account and the actor

#### Scenario: Password reset by code
- **WHEN** a Visitor resets the password of an account with a reset code
- **THEN** an event records "password reset" for that account, with that account as the actor

#### Scenario: Account deleted by the purge
- **WHEN** an account's deletion grace period ends and the account is deleted
- **THEN** an event records "account deleted" for that account with "system" as the actor

#### Scenario: Purge finds the deletion was cancelled
- **WHEN** the purge reaches an account whose Member cancelled deletion moments earlier
- **THEN** no "account deleted" event is recorded

#### Scenario: Operator bootstraps an admin
- **WHEN** the operator grants Super Admin with the bootstrap command
- **THEN** an event records "role changed" for that account with "operator" as the actor

#### Scenario: Setting the same role again
- **WHEN** a Super Admin sets a Member's role to the role they already have
- **THEN** no event is recorded

### Requirement: An event exists if and only if its action took effect
An audit event SHALL be stored in the same all-or-nothing step as the change it
describes. A refused, failed or interrupted action SHALL leave no event, and a
completed action SHALL always have one.

#### Scenario: Refused role change
- **WHEN** a Super Admin tries to remove their own Super Admin role and is refused
- **THEN** no event is recorded

#### Scenario: Last Super Admin protected
- **WHEN** someone tries to demote the last Super Admin and is refused
- **THEN** no event is recorded and the role is unchanged

#### Scenario: Database fails mid-action
- **WHEN** storing a role change fails part-way
- **THEN** neither the role change nor its event is stored, and the Super Admin sees an error

#### Scenario: Event cannot be stored
- **WHEN** the event can't be stored (for example its table is unavailable)
- **THEN** the action is not carried out either, and the actor sees an error

#### Scenario: Concurrent role changes
- **WHEN** two Super Admins change the same account's role at the same moment
- **THEN** there is exactly one event per change that took effect, and their order matches the order the changes were applied

### Requirement: The audit record cannot be changed and is kept for 7 years
Audit events SHALL NOT be editable or deletable through the application or its
database account, except that events older than 7 years SHALL be deleted
automatically. Deleting or anonymising an account SHALL NOT delete its events.

#### Scenario: Attempt to edit an event
- **WHEN** anything using the application's database account tries to change or delete an event younger than 7 years
- **THEN** the attempt is rejected and the event is unchanged

#### Scenario: Old events are removed
- **WHEN** an event is more than 7 years plus one day old
- **THEN** it is no longer stored

#### Scenario: Deleted account keeps its history
- **WHEN** a Member's account is deleted and anonymised
- **THEN** all events about that account remain and still refer to it

### Requirement: Audit events hold minimal personal data
Audit events SHALL NOT contain email addresses, names, passwords or hashes,
codes, tokens, client addresses or request bodies. People are identified only
by account.

#### Scenario: Events after a full account lifecycle
- **WHEN** a Member signs up, links Google, changes their password, exports their data, and deletes their account
- **THEN** none of the resulting events contains their email, name, password, any code or token, or a client address

### Requirement: Super Admins can read the full audit log
The admin area SHALL have an "Audit log" page, available only to Super Admins,
listing events newest first, 50 per page, showing time, kind, affected account,
actor and details. Accounts are shown by their current email, or as "Deleted
account" once anonymised. The list SHALL be filterable by affected or acting
account and by kind of event.

#### Scenario: Super Admin views the log
- **WHEN** a Super Admin opens the Audit log page
- **THEN** the most recent 50 events are shown newest first, with the total count

#### Scenario: Filter by account
- **WHEN** a Super Admin filters by one Member's account
- **THEN** only events where that account was affected or acted are shown

#### Scenario: Many events
- **WHEN** there are 100,000 events
- **THEN** any page loads within the same time budget as the admin user list, and the Super Admin can page through them

#### Scenario: No events yet
- **WHEN** a Super Admin opens the page and no events match
- **THEN** the page says there are no audit events, not an error

#### Scenario: Admin cannot read the log
- **WHEN** an Admin (not Super Admin) requests the audit log, in the UI or through the API
- **THEN** the request is refused and the admin area shows no Audit log link to them

#### Scenario: Member and Visitor cannot read the log
- **WHEN** a Member or a Visitor requests the audit log through the API
- **THEN** the request is refused

#### Scenario: Deleted account in the log
- **WHEN** an event's account has been deleted
- **THEN** it shows as "Deleted account", with no former email

#### Scenario: Mobile
- **WHEN** a Super Admin opens the page on a 360-pixel-wide screen
- **THEN** each event's time, kind, accounts and details are readable without horizontal scrolling of the whole page

### Requirement: Members can see their own security activity
The Settings page SHALL show a "Security activity" list of events whose
affected account is the signed-in Member, newest first, 20 at a time with a
way to load more. An actor that is the Member shows as "you", "system" as
"automatically", and any other account or the operator as "by an
administrator", without identifying which one.

#### Scenario: Member sees their password change
- **WHEN** a Member changes their password and opens Settings
- **THEN** "Password changed" by you appears at the top of Security activity

#### Scenario: Member sees a staff role change anonymously
- **WHEN** a Super Admin changes a Member's role to Admin
- **THEN** the Member's Security activity shows "Role changed from Member to Admin, by an administrator" and doesn't name the Super Admin

#### Scenario: Member sees sessions signed out after token reuse
- **WHEN** the system signs out all of a Member's sessions because a session token was reused
- **THEN** their Security activity shows that all sessions were signed out automatically

#### Scenario: Only their own events
- **WHEN** a Member loads their Security activity
- **THEN** it contains no events about any other account, and no request lets them read another account's activity

#### Scenario: New account
- **WHEN** a Member with no events opens Settings
- **THEN** Security activity says there is no activity yet

#### Scenario: Activity fails to load
- **WHEN** the activity request fails
- **THEN** the rest of Settings still works, and the Security activity section shows an error with a retry
