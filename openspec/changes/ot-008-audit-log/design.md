# Design

## Context

See proposal.md for why. Current state that shapes the approach:

- There's no cross-repository unit of work, and that was deliberate (see
  `ports/outgoing/account_lifecycle.rs`). Multi-table atomic steps run in one
  sqlx transaction inside a single adapter method: `save_role_change` (with
  the last-super-admin advisory lock) and `anonymize_account`. Most flows do
  several separate writes in order (for example `reset_password`: consume
  code, update password, revoke sessions).
- Role changes need `SuperAdminUser`, and listing users needs `AdminUser`.
  The admin group is under `require_auth`, `require_fresh_session` and
  `require_verified_email`.
- `just grant-admin` changes the role with raw SQL through `psql`.
- The app connects as the role that runs migrations, so it owns its tables.
- Frontend: `AdminPanel.svelte` (paginated user list, shadcn pagination) on
  `/dashboard/admin`, and `SettingsPanel.svelte` on `/dashboard/settings`. The
  Admin nav link only appears once the role is known.

## Goals / Non-Goals

**Goals:**
- Atomic event plus action, without introducing a unit of work.
- The database itself refuses changes, not just app discipline.

**Non-Goals:**
- Protection against the database owner or a superuser (dropping the trigger,
  restoring a backup). That needs a separate migration role or external
  write-once storage, which is out of scope.
- A generic event-sourcing or domain-event mechanism.

## Hypothesis

We believe that passing the event into the repository method that makes the
action's primary write, and inserting it in that method's transaction, gives
"event if and only if the action took effect" for every audited action. Slice
1 tests this on the hardest case, role changes. The insert must share the
transaction that holds the last-super-admin advisory lock and the
optimistic-version update. `#[sqlx::test]` cases prove it: a refused
last-super-admin demotion, a version conflict and an injected insert failure
each leave neither change nor event. We'll know it holds for every other
action when slice 2's integration tests cover each one the same way.

## Decisions

### D1. The event rides on the action's primary write (all layers)

- **Domain:** `AuditEvent { kind: AuditKind, subject: UserId, actor: Actor,
  details: AuditDetails }` with `Actor = User(UserId) | System | Operator`
  and closed enums for kind and details (`RoleChanged { from, to }`,
  `SignInMethod { provider }`, otherwise none). Plain data with no I/O. The
  type allows nothing free-form, so it can't carry personal data
  (spec: minimal personal data).
- **Application:** each audited use case builds the event and passes it to
  the repository method that makes the action's primary write. That means
  new or changed port methods:

  | Action | Port method (gets `event: &AuditEvent`) |
  |---|---|
  | role changed | `UserRepository::save_role_change` |
  | password changed / set | `UserRepository::save_audited` (new: `save` plus event) |
  | password reset | `UserRepository::update_password` |
  | deletion requested / cancelled | `UserRepository::save_audited` |
  | account deleted | `AccountLifecycleRepository::anonymize_account` (event only inserted when the scrub applies, so `Ok(false)` leaves no event) |
  | sign-in method linked / unlinked | `OAuthAccountRepository::save` / `delete_by_user_and_provider` |
  | sessions signed out after reuse | `RefreshTokenRepository::revoke_all_for_user` (new variant `revoke_all_for_user_audited`; the other callers stay unaudited) |
  | data exported | `AuditLogRepository::record` (standalone; nothing else is written) |

- **Infrastructure:** each adapter method opens or extends its transaction
  and inserts into `audit_events` before commit, through one shared
  `insert_audit_event(&mut tx, &AuditEvent, trace_id)` helper.
- The trace ID comes from the current span inside the helper (OT-001
  context), so the ports don't carry it.
- **Rejected: a separate `AuditLog::record` after the action.** It's simpler,
  but a crash or DB error between the two writes leaves an action with no
  event, which breaks the core guarantee.
- **Rejected: a unit of work threaded through ports.** Already rejected in
  this codebase for good reasons (sqlx and mockall friction).
- **Rejected: ambient staging.** The use case would call `audit.stage(event)`,
  and a shared adapter transaction helper would insert staged events into the
  next transaction to commit. No port signatures change, but the event lands
  in whichever transaction commits next. A write added before the primary one
  would carry it, and a write that bypasses the helper would drop it without
  any error. It hides task-local state in persistence. The explicit parameter
  costs more upfront, but the compiler enforces it and tests can see it.
- **Rejected: capture triggers on `users` and the other tables.** The app
  connects as one database user, so a trigger can't see the actor without a
  `SET LOCAL` on every transaction. It also can't see intent: change, set and
  reset of a password look identical, and anonymisation looks like edits. And
  exports and token-reuse sign-outs write nothing a trigger can catch.
- **Rejected: events collected on the `User` entity and flushed by `save`.**
  It hides a side effect in `save`, and it doesn't cover the actions that
  aren't user-row saves (OAuth links, session revocation, reset by user ID).

### D2. Table, append-only guard, retention (infrastructure, migration)

```sql
CREATE TABLE audit_events (
  id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  occurred_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
  kind        TEXT NOT NULL,          -- CHECK against the closed list
  subject_id  UUID NOT NULL,          -- no FK: events outlive any row
  actor_kind  TEXT NOT NULL CHECK (actor_kind IN ('user','system','operator')),
  actor_id    UUID,                   -- CHECK: set iff actor_kind = 'user'
  details     JSONB NOT NULL DEFAULT '{}',
  trace_id    TEXT
);
CREATE INDEX ON audit_events (subject_id, id DESC);
CREATE INDEX ON audit_events (actor_id, id DESC) WHERE actor_id IS NOT NULL;
CREATE INDEX ON audit_events (kind, id DESC);
```

- The identity `id` gives the order. Pages use `id DESC` (the spec's "order
  matches application order" comes from the insert happening inside the
  serialising transaction).
- **Guard:** a `BEFORE UPDATE OR DELETE` row trigger raises an exception
  unless it's a DELETE of a row with `occurred_at < now() - interval '7 years'`.
  A `BEFORE TRUNCATE` statement trigger always raises. This works even
  though the app owns the table. `REVOKE` alone doesn't stop an owner from
  re-granting.
- No FK to `users`. Accounts are anonymised, not deleted, today. Even if
  `AccountLifecycleRepository::delete` is used, events must survive.
- `kind` and `details` are validated on the way out too. An unknown kind
  read back maps to a generic "Other event", never a 500, so an older
  release can read a newer release's events.

### D3. Retention job (infrastructure, leader lock)

A daily `audit_retention` job under the leader lock in `main.rs`, with
OT-001 job metrics and the existing job-not-running alert, deletes events
past 7 years in batches of 10,000. For years it will delete nothing. It
exists now so the policy is enforced from day one and the trigger's delete
path is tested.

### D4. API (infrastructure, OpenAPI)

- `GET /api/v1/admin/audit-events?limit&offset&account_id&kind`: `SuperAdminUser`,
  `ApiQuery`, in the existing admin group. The response is
  `{ events: [...], total }` like the user list. Each event includes the
  subject's and actor's current email from a `LEFT JOIN users`, or `null`
  plus `deleted: true` once anonymised (the read model only, never stored).
  `limit` is capped at 100.
- `GET /api/v1/users/me/security-activity?limit&before_id`: the
  authenticated, verified, fresh-session profile group. Keyset pagination
  ("load more"). Only `subject_id = caller`. The actor is reduced on the
  server to `you | system | administrator`, so staff identities never reach
  the Member's browser.
- DTOs carry `kind` and `actor` as closed string enums so the OpenAPI
  description lists them (api spec). Run `just api-types`.

### D5. Frontend

- New page `/dashboard/audit` with an `AuditLogPanel` island (`client:load`),
  reusing AdminPanel's pagination pattern and stale-response guard. A
  filter for account (by ID, picked from a user-list link) and a kind
  select. The nav link is shown only once the role is known to be
  `super_admin` (add `isSuperAdminRole`), never guessed.
- A `SecurityActivity` section inside `SettingsPanel`, loaded with its own
  request, error and retry, so a failure never blocks the rest of Settings
  (dashboard rules). Human labels per kind live in one map shared by both
  views.

### D6. Operator bootstrap

`just grant-admin` becomes one SQL statement: a CTE that updates the role
and inserts the `role_changed` event (`actor_kind = 'operator'`, old and new
role) in one statement, only when the role actually changed.

## Risks / Trade-offs

- [The table owner can drop the trigger] → Accepted. A new migration doing
  that would show up in review. A separate, less-privileged runtime role is
  a later hardening step.
- [Port signature churn touches many mocks] → Done per slice, one action
  family at a time. mockall expectations gain one argument.
- [Some flows stay multi-step (reset password: consume code, then update
  password with event, then revoke sessions)] → The event marks the primary
  effect (the password changed), which is atomic with its event. A later
  step failing is the same partial state the flow already tolerates today.
- [7 years of events grow the table] → It's tiny at this scale (a few events
  per Member per year). The indexes keep paging fast. Partitioning can wait.
- [The email shown in the admin view is today's email, not the one at the
  time] → Intended. Storing the email at the time would put personal data in
  a 7-year record.

## Migration Plan

Slice 1: one additive migration (table, indexes, trigger functions,
triggers). It's safe with the previous release, which never touches the
table. Rollback: redeploy the previous digest. The table stays and is unused.
Contracting is never needed, because the table is permanent. `just
db-prepare` and commit `backend/.sqlx/` after the queries change. Later
slices add no schema beyond widening the `kind` CHECK, which is safe with the
previous release because it only accepts more values.
