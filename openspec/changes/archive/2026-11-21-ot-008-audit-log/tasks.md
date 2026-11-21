# Tasks

## 1. Slice 1 (first slice): role changes, recorded atomically and shown to Super Admins

Delivers these scenarios: Super Admin changes a role; Operator bootstraps an admin;
Setting the same role again; Refused role change; Last Super Admin protected;
Database fails mid-action; Event cannot be stored; Concurrent role changes;
Attempt to edit an event; Super Admin views the log; Filter by account; Many
events; No events yet; Admin cannot read the log; Member and Visitor cannot
read the log; Mobile (audit page).

- [x] 1.1 Migration: `audit_events` table, indexes, the `BEFORE UPDATE OR DELETE` guard (deletes allowed only past 7 years) and the `BEFORE TRUNCATE` guard (D2). `#[sqlx::test]` cases: update rejected, delete of a fresh row rejected, delete of a back-dated row (inserted with `occurred_at` 7 years + 1 day ago) allowed, truncate rejected. Rollback plan: redeploy the previous digest. The table is additive and unused by it.
- [x] 1.2 Domain: `AuditEvent`, `AuditKind`, `Actor`, `AuditDetails` as closed types with no I/O (D1). Unit tests for the actor and details invariants (`actor_id` only for `Actor::User`, `RoleChanged` details only for that kind).
- [x] 1.3 Infrastructure: the `insert_audit_event(&mut tx, …)` helper that takes the trace ID from the current span. `save_role_change` takes `&AuditEvent` and inserts it inside its existing transaction, after the last-super-admin check and the versioned update. `#[sqlx::test]`: a successful change writes exactly one event with old and new role; a last-super-admin refusal, a version conflict and an injected insert failure (for example a temporarily stricter CHECK) each leave neither role change nor event; two concurrent changes give events in applied order. Run `just db-prepare` and commit `backend/.sqlx/`.
- [x] 1.4 Add a coverage guard test: a test-only function with an exhaustive `match` on `AuditKind` (no wildcard arm) that maps each kind to the `#[sqlx::test]` function proving that kind is written atomically (event if and only if the action took effect). A new kind with no entry fails to compile, and an entry naming a test that doesn't exist fails to compile too. A comment on `AuditKind` points to this guard. Verify by adding a dummy variant locally and checking that `just test` fails, then remove it.
- [x] 1.5 Application: `SetUserRoleUseCase` builds the event and passes it on. No event on the same-role path (`save`) or on any refusal. Use-case tests with mocks cover each branch.
- [x] 1.6 `just grant-admin`: a single-statement CTE that updates the role and inserts an `operator` event only when the role changed (D6). Verify locally: run it twice, and the second run adds no event.
- [x] 1.7 API: `AuditLogRepository::list(filter, limit, offset)` plus count, `GET /api/v1/admin/audit-events` behind `SuperAdminUser` with `ApiQuery`, closed-enum DTOs, current email or `deleted` from a join (D4). Integration tests: Super Admin 200 with total; Admin, Member and Visitor refused; filter by account matches subject or actor; empty result; `limit` capped. Run `just api-types` and commit `backend/openapi.json` and `schema.d.ts`.
- [x] 1.8 Web: the `/dashboard/audit` page with an `AuditLogPanel` (`client:load`), pagination like AdminPanel, account and kind filters, empty and error states, and a nav link only for a known `super_admin` (D5). Vitest browser tests: the list, the filters, empty, error, and no link for Admin or unknown role. Playwright flow with the API mocked: a Super Admin changes a role, then sees it in the audit log. Check at 360 px.
- [x] 1.9 Seed 100,000 events locally and confirm with the existing load or bench tooling that the audit list page latency is in line with the admin user list. Record the numbers in design.md Hypothesis.
- [x] 1.10 Add a log line and an error-rate check for event insert failures: a failed insert surfaces as the action's 5xx, which the existing 5xx alert covers. Confirm the failure appears with its trace ID in the logs.
- [x] 1.11 `just ci` green. Note for the first deploy: migration runs at boot, then `just smoke <url>`, then a role change checked on the audit page.

## 2. Slice 2: the other account actions, and retention

Delivers these scenarios: Member changes their password; Password reset by
code; Account deleted by the purge; Purge finds the deletion was cancelled;
Old events are removed; Deleted account keeps its history; Events after a
full account lifecycle; and the remaining kinds of "Account actions are
recorded".

- [x] 2.1 After slice 1, revise design.md with what it showed (D7) and add the remaining slices here.
- [x] 2.2 Migrations: widen `audit_events_kind_check` to the new kinds (`password_changed`, `password_set`, `password_reset`, `sign_in_method_linked`, `sign_in_method_unlinked`, `deletion_requested`, `deletion_cancelled`, `account_deleted`, `data_exported`, `sessions_revoked_after_reuse`) as `NOT VALID`, then validate it in a second migration (D7).
- [x] 2.3 Domain: the new `AuditKind`s and `AuditDetails::SignInMethod { provider }`, with a constructor per kind. Unit tests for the details invariant. `AuditKindName` and `AuditKindFilter` DTOs, the web label map and the filter list follow. Run `just api-types`.
- [x] 2.4 Passwords: `UserRepository::save_audited(&User, &AuditEvent)` (the `save` write plus the event, one transaction) for change and set; `update_password` takes the event for reset. Use-case mock tests per branch (wrong current password, OAuth-only account, used reset code: no event). `#[sqlx::test]` atomicity cases, and their entries in the guard.
- [x] 2.5 Sign-in methods: `OAuthAccountRepository::save` and `delete_by_user_and_provider` take the event. Linked on explicit link, hybrid auto-link and Google signup; unlinked only when a row was deleted; the last-method refusal leaves none. Mock and atomicity tests, guard entries.
- [x] 2.6 Deletion: request and cancel through `save_audited`; `anonymize_account` takes the `account_deleted` event (actor `system`) and inserts it only when the scrub applies. `#[sqlx::test]`: cancel moments before the purge gives no `account_deleted` event; the deleted account's earlier events remain and show as "Deleted account". Guard entries.
- [x] 2.7 Export: `AuditLogRepository::record(&AuditEvent)`, called by `ExportUserDataUseCase` before it returns the data; a failed record fails the export (D7). Tests and guard entry.
- [x] 2.8 Token reuse: `RefreshTokenRepository::revoke_all_for_user_audited` (revocation and event in one transaction, actor `system`); the other callers stay unaudited. Tests and guard entry.
- [x] 2.9 Retention (D3, revised): a step in the hourly `database_maintenance` pass under the leader lock, deleting events past 7 years in batches of 1,000, covered by its job metrics and `JobNotSucceeding`, with an `occurred_at` index. `#[sqlx::test]`: back-dated events go, younger ones stay, and a pass with nothing to delete succeeds.
- [x] 2.10 Minimal personal data: an integration test that runs signup, Google link, password change, export and deletion for one Member, then checks that no event row (as text) contains their email, name, password, any code or token, or a client address.
- [x] 2.11 `just ci` green. Note for the first deploy: both migrations run at boot; check one password change on the audit page.

## 3. Slice 3: Members' Security activity

Delivers these scenarios: Member sees their password change; Member sees a
staff role change anonymously; Member sees sessions signed out after token
reuse; Only their own events; New account; Activity fails to load.

- [x] 3.1 API (D4): `AuditLogRepository::list_for_subject(subject, limit, before_id)` and `GET /api/v1/users/me/security-activity?limit&before_id` in the profile group (signed in, verified, fresh session), `ApiQuery`, keyset paging, 20 per page. The actor is reduced on the server to `you`, `system` or `administrator`. Integration tests: only the caller's events; a role change by a Super Admin shows no staff account ID or email anywhere in the body; paging with `before_id`; Visitor refused. Run `just api-types`.
- [x] 3.2 Web: a Security activity section in `SettingsPanel`, loaded by its own request with its own error and retry, so the rest of Settings works when it fails; "No activity yet" when empty; "Load more". Labels from `lib/audit.ts`. Vitest: list, actor phrases, empty, error and retry with the rest of Settings still usable, load more. Playwright with the API mocked: a Member changes their password and sees it at the top. Check at 360 px.
- [x] 3.3 `just ci` green.

## 4. Feature done

- [x] 4.1 Document the audit log (what's recorded, the 7-year retention, the append-only guard and its limits, the `grant-admin` change, Members' Security activity) in `backend/ARCHITECTURE.md` and `README.md`.
- [x] 4.2 Archive the change.
