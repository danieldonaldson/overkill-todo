# Design

## Context

See proposal.md for why and the spec deltas for behaviour. What shapes the
approach:

- **Static frontend.** Astro can't build a page per list. The web server's
  fallback answers any unknown path with the site root, which redirects to
  `/dashboard`, so list addresses need a rewrite.
- **In-site navigation.** `ClientRouter` keeps JavaScript modules alive between
  dashboard pages. Panels are rebuilt on each move; the header is persisted.
- **No unit of work.** Multi-row atomic steps run in one transaction inside one
  adapter method (`ports/outgoing/account_lifecycle.rs`).
- **Purge** anonymises the `users` row and keeps it, so a foreign key cascade
  alone never erases a Member's lists.
- **Errors.** `AppError::Conflict` carries only a message, so it can't return
  the current version of a todo.
- **Messages.** The sonner toaster component exists but isn't mounted anywhere.

## Goals / Non-Goals

**Goals:**
- One rule for concurrent edits that still works when the other editor is
  another person.
- Every list write is authorised, limited and ordered under one lock.
- A save can be repeated safely, so a lost answer never needs a guess.

**Non-Goals:**
- Merging fields, offline queues or CRDTs. A conflict is shown, not merged.
- Deciding how sharing grants access. D3 keeps ownership in one place for it.
- Virtualised rendering. A list holds at most 1,000 todos.

## Hypothesis

We believe optimistic edits are safe if (a) each queued edit remembers the
version the Member saw, (b) only that tab's own successful saves may advance
it, and (c) every save is repeatable. A refetch must never advance a queued
edit, because that would turn someone else's change into a silent overwrite.

**Slice 1 tests this** with create, add and tick, before any ordering work:

- A seeded random test runs two copies of the client queue against an in-memory
  server that follows D4's rules. It randomly delays, fails and loses answers,
  and mixes in refetches. Once the connection recovers and both queues drain,
  both tabs must match the server, no add may be duplicated, and no accepted
  edit may have overwritten a version its tab had not seen.
- `#[sqlx::test]` cases run the same version and repeat rules against the real
  repository, including concurrent transactions.
- A two-tab flow in `tests/integration/` runs against the real backend.

It holds if the random test passes for 10,000 seeds, and fails when (b) is
deliberately broken by letting a refetch advance a queued edit.

## Decisions

### D1. One version per todo and per list (domain)

`version INTEGER` starts at 1 and goes up when a todo's title or done state
changes, or a list's name changes. Moves don't change it. A request that asks
for the state already stored succeeds without changing the version.

- **Rejected: per-field versions.** A rename and a tick in different tabs would
  both succeed, but each todo carries several versions and the Member's "I saw
  this" becomes ambiguous. The specs choose the simpler rule.
- **Rejected: `updated_at` as the version.** Clock precision, and moves would
  count.
- **Rejected: last write wins.** It breaks the proposal's main promise.

### D2. Tables (persistence-postgres, migration)

```sql
CREATE TABLE lists (
  id           UUID PRIMARY KEY,
  user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
  creation_key UUID NOT NULL,
  name         TEXT NOT NULL,
  version      INTEGER NOT NULL DEFAULT 1,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
  UNIQUE (user_id, creation_key)
);
CREATE INDEX ON lists (user_id, created_at, id);

CREATE TABLE todos (
  id           UUID PRIMARY KEY,
  list_id      UUID NOT NULL REFERENCES lists (id) ON DELETE CASCADE,
  creation_key UUID NOT NULL,
  title        TEXT NOT NULL,
  version      INTEGER NOT NULL DEFAULT 1,
  position     BIGINT,          -- open todos only
  completed_at TIMESTAMPTZ,     -- done todos only
  created_at   TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
  CHECK ((position IS NULL) = (completed_at IS NOT NULL)),
  UNIQUE (list_id, creation_key),
  UNIQUE (list_id, position) DEFERRABLE INITIALLY IMMEDIATE
);
```

The check makes "open with a position" and "done with a time" the only states.
There are no content checks in SQL: a failed check makes Postgres log the whole
row, title included (D9). The position constraint is deferrable so that one
statement can renumber a list.

### D3. The server makes IDs; the client makes creation keys

A create request carries a random `key`. The server generates the ID, and a
repeat with the same key returns the existing row, checked before the limit.
In the browser a new item is tracked by its key, and the queue fills in the
server's ID when it sends the next edit.

- **Rejected: client-made IDs.** A collision response would let anyone who was
  once sent a list's address probe whether it still exists.
- Cost: a new list has no address until its creation is answered. It is shown
  at once, and its address replaces `/dashboard/lists` when the answer comes.

### D4. Every list write locks its list first (persistence-postgres)

Each write opens a transaction and starts with
`SELECT … FROM lists WHERE id = $1 AND user_id = $2 FOR UPDATE`. No row means
404, for missing and foreign lists alike. The todo is then always selected by
`id AND list_id`. This one step gives ownership, limits that hold under
concurrency, and one move at a time. When sharing arrives, only this lookup
changes.

Creating a list locks the `users` row instead (and requires
`deleted_at IS NULL`), so the list limit holds and a purge can't be followed by
a new list from a token that hasn't expired. Lock order is always users →
lists → todos, the purge included.

An edit with a stale version is compared with the stored row under the lock:
same state → success; otherwise → conflict with the current row.

- **Rejected: a bare `UPDATE … WHERE version = $n`.** Enough for a rename, but
  not for limits, order or a consistent "current" in the answer.
- **Rejected: serialisable transactions.** Retries everywhere, for lists that
  rarely see two writers.

### D5. Routes and answers (application, api)

All under `/api/v1/lists`, with `require_verified_email()` and `require_auth()`,
above `require_csrf()`:

| Request | Body or query | Answer |
|---|---|---|
| `GET /lists` | | lists with open counts |
| `POST /lists` | `key`, `name` | the list |
| `GET /lists/{id}` | | the list and all its todos |
| `PATCH /lists/{id}` | `version`, `name` | the list |
| `DELETE /lists/{id}` | `version`, `todo_count` | 204 |
| `POST /lists/{id}/todos` | `key`, `title` | the todo |
| `PATCH /lists/{id}/todos/{todo_id}` | `version`, exactly one of `title`, `done` | the todo |
| `DELETE /lists/{id}/todos/{todo_id}` | `version` | 204 |
| `POST /lists/{id}/todos/{todo_id}/move` | `above`, `below` (D6) | open todo IDs in order |

- `version` and `todo_count` are required, so a request without them gets the
  existing JSON 400.
- **Exactly one field per PATCH** matters for repeats. If a stale edit finds
  the state it wanted at exactly its version + 1, nothing else can have
  changed, so the client may carry on from there (D7).
- Stale edits answer 409 with `code: "stale"` and `current`. Limits answer 409
  with `code: "limit_reached"` and a message naming the limit. 404s carry
  `list_not_found` or `todo_not_found`. The client treats a 404 on a delete as
  success.
- Use cases return an outcome (`Applied`, `Stale(current)`, `LimitReached`)
  rather than stretching `DomainError`; the handler maps it to a status.
- `PATCH` is added to the CORS methods.
- No `require_fresh_session()`: like other ordinary routes, these trust the
  access token until it expires.
- **Validation:** `ListName` and `TodoTitle` in the domain trim, count code
  points, and refuse control characters and U+2028/U+2029. The rule counts
  after trimming, which `#[validate(length)]` can't express. The schema states
  it as a pattern; if the fuzzer still disagrees, those operations skip that
  check in `apifuzz/schemathesis.toml`, with the reason. Their `Debug` output
  hides the text.

### D6. Order is a gapped integer, renumbered when a gap runs out (domain, persistence-postgres)

Open todos have positions 2^20 apart. A move takes the midpoint of its
neighbours; if there is none, the list's open todos are renumbered in one
statement, under the list lock. The midpoint rule is a pure domain function, so
it is property-tested.

A move sends up to 20 open todo IDs above the target position and 20 below,
nearest first. The server ignores any that aren't open in this list, including
foreign IDs, and then places the todo directly below the first remaining ID in
`above`, else directly above the first remaining one in `below`, else at the
bottom. An empty `above` means the top. Moving a done todo answers `stale`.

- **Rejected: fractional string keys.** They grow without bound, and their main
  benefit, ordering without a lock, isn't needed.
- **Rejected: target index.** It means a different place once the list changes.
- **Rejected: renumbering on every move.** Up to 1,000 row writes per drag.
- **Accepted:** if all 40 neighbours disappear, the todo goes to the bottom.

### D7. The client is confirmed state plus a queue (web, `lib/lists/`)

A module-level runes store holds what the server last confirmed and a queue of
the Member's unsaved operations. The screen is the queue applied to the
confirmed state, so undoing a failed save is removing it from the queue.

- **One queue per tab**, one request at a time, in the order made. Refetches
  go through the same queue, so they never overlap a save.
- **Versions:** an operation stores the version on screen when the Member
  acted. When a save moves a todo from version n to n + 1, later queued
  operations on that todo at n move to n + 1. Nothing else changes a stored
  version: not a refetch, and not a success whose version isn't n + 1.
- **409 or 404:** the operation and later queued ones on the same item are
  dropped, the current version is shown (or the item removed), and the Member
  sees one message. Dropped text stays available to submit again.
- **400 or limit:** the same, without changing the confirmed item.
- **Network failure, 15-second timeout or 5xx:** the outcome is unknown. The
  save is repeated once; if that fails too, it is dropped as above, and the
  list is refetched as soon as the server answers again (on `online`, focus or
  the next action).
- **List deletion** waits for the server, because it is permanent.
- **Out-of-order loads:** an answer is applied only to the list it names.
- **Leaving:** the store survives in-site moves. `beforeunload` asks to confirm
  while the queue isn't empty. The store is in memory only, and is cleared on
  sign-out and when the account changes.
- **Messages** use the sonner toaster, mounted in the persisted header, so a
  failure is seen and announced on any dashboard page.

Alternatives: **TanStack Query** gives caching and rollback snapshots, but a
snapshot rollback loses later pending edits, and it has no notion of (b) in the
Hypothesis. **Disabling a row while it saves** is simple, but not optimistic.

### D8. List addresses (web, web image)

`pages/dashboard/lists/index.astro` is the overview and
`pages/dashboard/lists/[id].astro` builds one shell, from a placeholder ID. The
web server rewrites `/dashboard/lists/<anything>` to that shell. A small Vite
plugin does the same for `astro dev` and `astro preview`, so Playwright sees
the real addresses. The panel reads the ID from the address; one that isn't a
UUID shows "list not found" without a request.

- The `[id]` file name makes the existing page-template test expect
  `/dashboard/lists/[id]`, the name the specs use.
- **Rejected: `/dashboard/lists?id=…`.** No rewrite needed, but the address is
  worth being a path, and query strings are stripped from telemetry by design.
- **Slice 1 checks early** that the plugin works in `astro preview`. If not,
  Playwright runs against the web server configuration instead.

### D9. Keeping content and list IDs out of telemetry (web, application)

- **Page template:** the browser maps list addresses to `/dashboard/lists/[id]`
  before sending, and the server's `page_template` does so again.
- **Scrubbing:** a new rule, in both copies and the shared fixture, replaces
  the ID in `/dashboard/lists/<id>` anywhere in a message, stack or source.
- **API failures** pass `route`, so reports hold the route template.
- **Analytics:** a before-send hook in `analytics.js` rewrites the page and
  referrer. List pages keep the fixed title "Lists". If the deployed Umami
  version has no such hook, automatic tracking is turned off and the loader
  sends page views itself.
- **Backend:** spans and logs carry list, todo and user IDs only. `skip_all`
  everywhere, and the `Debug` rule in D5.
- **Postgres logs:** errors can log row values in their DETAIL line. Validation
  and locking mean the constraints shouldn't fire; `log_error_verbosity=terse`
  is set in the local and production compose files as a second net.
- A test writes marker strings through every route, including refused and
  failed saves, and searches the captured logs and spans for them.

### D10. Export and erasure (application, persistence-postgres)

- **Purge:** `anonymize_account` deletes the Member's lists in its existing
  transaction, so a failure leaves the account due and the next run retries.
- **Sweep:** each purge run also deletes lists whose account is already
  deleted. Without it, a purge by the previous release, during a roll or after
  a rollback, would leave lists behind for good.
- **Export:** `lists` is added to the export response. The worst case (100,000
  titles of 500 characters) is about 200 MB, too much to build in memory. Slice
  1 builds it in memory; before the feature is done, the handler streams the
  body one list at a time. A failure part-way aborts the response, so the
  browser reports a failed download rather than saving a partial file.

### D11. No feature flag

There is no production deploy, and slice 1 is usable on its own. The Lists
navigation entry is added as slice 1's last step.

### D12. Threat model

A new property, P-33, covers the `security` delta. Its checks are route tests
that try every list operation as another Member, an Admin, a Super Admin, an
unverified Member and a Visitor, comparing each 404 with that of an unknown ID.
The existing route-guard walk covers the Visitor case for future routes.

## Risks / Trade-offs

- [Repeating a lost add after the todo was deleted in another tab recreates it]
  → Only within the single immediate repeat; the Member can delete it again.
- [A request the browser gave up on is applied after the refetch] → The next
  edit gets a 409 with the current version, so nothing is overwritten.
- [Repeating a lost move can undo a later move from another tab] → Both end on
  one order, which is what the specs promise for moves.
- [One queue per tab means a slow save delays the rest] → Requests are small,
  and the timeout and single repeat bound it at about 30 seconds.
- [A list write lock serialises a shared list's editors later] → Transactions
  are a few short statements. Measured in slice 1 with a load scenario.
- [1,000 rows make drag or keyboard moves slow] → `content-visibility: auto`,
  and an Interaction to Next Paint check on a full list in the ordering slice.
- [Touch scrolling starts a drag] → Only a drag handle starts one, with
  `touch-action: none` on the handle alone.

## Migration Plan

1. **Expand:** one migration creates both empty tables. The previous release
   doesn't use them.
2. **Deploy:** backend, then web. The web release needs the new routes.
3. **Contract:** nothing to remove.

**Rollback:** redeploy the previous images. The tables stay, unused, and
Members' lists reappear when the release returns. Accounts purged meanwhile are
cleaned up by D10's sweep. Until then the rolled-back release also leaves lists
out of exports, so a long rollback needs a manual answer to export requests.

## Open Questions

- Whether a Quint model of D7 is worth adding. The random test covers the same
  interleavings; decide before sharing, when more than one person edits.
