# Tasks

## 1. Slice 1 (first slice): role changes, recorded atomically and shown to Super Admins

Delivers these scenarios: Super Admin changes a role; Operator bootstraps an admin;
Setting the same role again; Refused role change; Last Super Admin protected;
Database fails mid-action; Event cannot be stored; Concurrent role changes;
Attempt to edit an event; Super Admin views the log; Filter by account; Many
events; No events yet; Admin cannot read the log; Member and Visitor cannot
read the log; Mobile (audit page).

- [ ] 1.1 Migration: `audit_events` table, indexes, the `BEFORE UPDATE OR DELETE` guard (deletes allowed only past 7 years) and the `BEFORE TRUNCATE` guard (D2). `#[sqlx::test]` cases: update rejected, delete of a fresh row rejected, delete of a back-dated row (inserted with `occurred_at` 7 years + 1 day ago) allowed, truncate rejected. Rollback plan: redeploy the previous digest. The table is additive and unused by it.
- [ ] 1.2 Domain: `AuditEvent`, `AuditKind`, `Actor`, `AuditDetails` as closed types with no I/O (D1). Unit tests for the actor and details invariants (`actor_id` only for `Actor::User`, `RoleChanged` details only for that kind).
- [ ] 1.3 Infrastructure: the `insert_audit_event(&mut tx, …)` helper that takes the trace ID from the current span. `save_role_change` takes `&AuditEvent` and inserts it inside its existing transaction, after the last-super-admin check and the versioned update. `#[sqlx::test]`: a successful change writes exactly one event with old and new role; a last-super-admin refusal, a version conflict and an injected insert failure (for example a temporarily stricter CHECK) each leave neither role change nor event; two concurrent changes give events in applied order. Run `just db-prepare` and commit `backend/.sqlx/`.
- [ ] 1.4 Add a coverage guard test: a test-only function with an exhaustive `match` on `AuditKind` (no wildcard arm) that maps each kind to the `#[sqlx::test]` function proving that kind is written atomically (event if and only if the action took effect). A new kind with no entry fails to compile, and an entry naming a test that doesn't exist fails to compile too. A comment on `AuditKind` points to this guard. Verify by adding a dummy variant locally and checking that `just test` fails, then remove it.
- [ ] 1.5 Application: `SetUserRoleUseCase` builds the event and passes it on. No event on the same-role path (`save`) or on any refusal. Use-case tests with mocks cover each branch.
- [ ] 1.6 `just grant-admin`: a single-statement CTE that updates the role and inserts an `operator` event only when the role changed (D6). Verify locally: run it twice, and the second run adds no event.
- [ ] 1.7 API: `AuditLogRepository::list(filter, limit, offset)` plus count, `GET /api/v1/admin/audit-events` behind `SuperAdminUser` with `ApiQuery`, closed-enum DTOs, current email or `deleted` from a join (D4). Integration tests: Super Admin 200 with total; Admin, Member and Visitor refused; filter by account matches subject or actor; empty result; `limit` capped. Run `just api-types` and commit `backend/openapi.json` and `schema.d.ts`.
- [ ] 1.8 Web: the `/dashboard/audit` page with an `AuditLogPanel` (`client:load`), pagination like AdminPanel, account and kind filters, empty and error states, and a nav link only for a known `super_admin` (D5). Vitest browser tests: the list, the filters, empty, error, and no link for Admin or unknown role. Playwright flow with the API mocked: a Super Admin changes a role, then sees it in the audit log. Check at 360 px.
- [ ] 1.9 Seed 100,000 events locally and confirm with the existing load or bench tooling that the audit list page latency is in line with the admin user list. Record the numbers in design.md Hypothesis.
- [ ] 1.10 Add a log line and an error-rate check for event insert failures: a failed insert surfaces as the action's 5xx, which the existing 5xx alert covers. Confirm the failure appears with its trace ID in the logs.
- [ ] 1.11 `just ci` green. Note for the first deploy: migration runs at boot, then `just smoke <url>`, then a role change checked on the audit page.

## 2. Feature done

- [ ] 2.1 After slice 1 ships, revise design.md with what it showed, then add the remaining slices here: (a) the other account actions (password changed, set and reset; sign-in method linked and unlinked; deletion requested, cancelled and carried out; data exported; sessions signed out after reuse) plus the retention job (D3); (b) Members' Security activity on Settings. Every new kind gets its entry in the 1.4 guard and its atomicity test.
- [ ] 2.2 Document the audit log (what's recorded, the 7-year retention, the append-only guard and its limits, the `grant-admin` change) in `backend/ARCHITECTURE.md` and `README.md`.
- [ ] 2.3 Archive the change.
