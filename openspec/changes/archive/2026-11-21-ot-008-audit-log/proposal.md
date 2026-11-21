# OT-008: Audit log

## Why

As a Super Admin, I want a permanent, tamper-resistant record of who changed
roles and what happened to each account, so that I can answer "who did this,
and when?" months or years later. As a Member, I want to see recent security
activity on my account, so that I notice a change I didn't make.

Today the only record of a role change, password change or account deletion is
a log line, and logs are kept for 30 days (OT-001). The admin user list (just
paginated) makes role changes routine, and account deletion is a GDPR-relevant
action. Both need a record that outlives telemetry, can't be edited by the
app, and holds no more personal data than it must.

## What Changes

- Every privileged or security-relevant account action is recorded as an
  audit event: role changed (by an Admin, a Super Admin or the operator's
  bootstrap command), password changed, set or reset, a sign-in method linked
  or unlinked, deletion requested, cancelled or carried out, data exported,
  and all sessions signed out after a reused session token.
- An event is recorded if and only if its action took effect. A failed or
  refused action leaves no event, and a successful one can't lose its event.
- Events can't be changed or deleted by the app. Events older than 7 years
  are deleted automatically, and nothing else is ever deleted.
- Events identify people by account, never by email, name, password, code,
  token or client address. A deleted Member's events stay, attached to the
  anonymised account.
- Super Admins get an "Audit log" page in the admin area: newest first,
  paginated, filterable by account and by kind of event. Admins and Members
  can't see it.
- Members get a read-only "Security activity" list on Settings showing events
  on their own account. Changes made by staff show as "by an administrator",
  without saying which one.

## Capabilities

### New Capabilities
- `audit`: what account and privileged actions are recorded, the guarantees
  on that record (complete, unchangeable, minimal personal data, 7-year
  retention), and who can read which part of it.

### Modified Capabilities
None.

## Appetite

Two weeks.

## Out of Scope

- Email-address changes. There's no flow for them yet. When one is added, it
  must record an event under this capability.
- Sign-ins, failed sign-ins and code guesses. Those are telemetry (OT-007),
  not audit.
- Exporting the audit log, or shipping it to external or write-once storage.
- Recording the client address or device of an action.
- Members' to-do content changes.

## Dependencies

- A new table and a forward-only migration (additive, safe for the previous
  release).
- The operator's admin bootstrap command changes to record its event.
- No production deploy exists yet. Deploy checks are recorded for the first
  deploy.

## Ready

- **Riskiest unknown:** recording each event in the same database transaction
  as its action, inside a repository design that deliberately has no
  cross-repository unit of work, while also making the table impossible to
  change from the app except for deleting rows past retention.
- **First slice:** role changes end to end: recorded atomically (a refused or
  failed change leaves nothing), protected against update and delete at the
  database level, and shown on the Super Admin audit page.
