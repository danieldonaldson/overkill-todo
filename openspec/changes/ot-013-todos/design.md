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
- Every list guarantee names the strongest check that fits it, and a gate fails
  when one is missing or has drifted.

**Non-Goals:**
- Merging fields, offline queues or CRDTs. A conflict is shown, not merged.
- Deciding how sharing grants access. D3 keeps ownership in one place for it.
- Virtualised rendering. A list holds at most 1,000 todos.
- Proving rendering, layout, CSS, SQL text, HTTP wiring or third-party code;
  those guarantees name their tests and why no proof or model applies.

## Hypothesis

We believe optimistic edits are safe if (a) each queued edit remembers the
version the Member saw, (b) only that tab's own successful saves may advance
it, and (c) every save is repeatable. A refetch must never advance a queued
edit, because that would turn someone else's change into a silent overwrite.

**Slice 1 tests this** with create, add and tick, before any ordering work:

- A seeded random test runs two copies of the client queue against an in-memory
  server that follows D4's rules. It randomly delays, fails and loses answers,
  and mixes in refetches. Throughout, no add may be duplicated and no accepted
  edit may have overwritten a version its tab had not seen. Once the connection
  recovers and both queues drain, neither tab may show a change the server
  doesn't have as its own. Tabs aren't live, so they match the server only
  after a load: the test then refetches once in each tab and requires both to
  match it exactly.
- `#[sqlx::test]` cases run the same version and repeat rules against the real
  repository, including concurrent transactions.
- A two-tab flow in `tests/integration/` runs against the real backend.

It holds if the random test passes for 10,000 seeds, and fails when (b) is
deliberately broken by letting a refetch advance a queued edit.

**Result (slice 1):** passes for 10,000 seeds (`VITE_HYPOTHESIS_SEEDS=10000`);
the broken variant (`refetchAdvancedQueuedVersion(true)`, which lets a refetch
advance a queued version) fails. The real two-tab flow reaches a stale 409 when
the second tab ticks a todo after the first tab changed it twice, and both tabs
then show the server's current state.

A review after slice 1 found that this test can't see wrong order, lost edits or
endless loads, because it refetches before comparing. `regressions.test.ts` adds
a one-tab random test without faults that compares with no refetch and allows no
conflict message. The two-tab test now also fails some loads, and still passes
for 10,000 seeds.

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
  at once, and its address replaces `/dashboard/lists` when the answer comes,
  with `history.replaceState`, keeping the router's own `history.state`. Not
  `navigateReplace()`: that is a full load, which would discard the queue.

### D4. Every list write locks its list first (persistence-postgres)

Each write opens a transaction and starts with
`SELECT … FROM lists WHERE id = $1 AND user_id = $2 FOR UPDATE`. No row means
404, for missing and foreign lists alike. The todo is then always selected by
`id AND list_id`. This one step gives ownership, limits that hold under
concurrency, and one move at a time. When sharing arrives, only this lookup
changes.

Creating a list locks the `users` row instead (and requires
`deleted_at IS NULL`), so the list limit holds and a purge can't be followed by
a new list from a token that hasn't expired. For a deleted account it answers
401 `token_invalid`, so the client refreshes, fails, and goes to sign-in. Lock order is always users →
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
| `GET /lists` | | lists, oldest first |
| `POST /lists` | `key`, `name` | the list |
| `GET /lists/{id}` | | the list and all its todos |
| `PATCH /lists/{id}` | `version`, `name` | the list |
| `DELETE /lists/{id}` | `version`, `todo_count` | 204 |
| `POST /lists/{id}/todos` | `key`, `title` | the todo |
| `PATCH /lists/{id}/todos/{todo_id}` | `version`, exactly one of `title`, `done` | the todo |
| `DELETE /lists/{id}/todos/{todo_id}` | `version` | 204 |
| `POST /lists/{id}/todos/{todo_id}/move` | `above`, `below` (D6) | open todo IDs in order |

Every list in an answer carries `id`, `name`, `version`, `open_count` and
`todo_count`. Todos carry no position: array order is the contract (open todos
in order, then done ones, most recently completed first).

- `version` and `todo_count` are required, so a request without them gets the
  existing JSON 400.
- **Exactly one field per PATCH** matters for repeats. If a stale edit finds
  the state it wanted at exactly its version + 1, nothing else can have
  changed, so the client may carry on from there (D7).
  It is an untagged enum of two objects that refuse unknown fields, so the API
  description says `oneOf` and a body with both fields is invalid there too.
- **Deleting a list** is offered only on the open list, where every todo is
  loaded, and is sent after the queue drains. Its 409 carries the current name,
  version and `todo_count`, so the Member is asked again without a refetch.
- Stale edits answer 409 with `code: "stale"` and `current`, declared in the
  DTOs. Limits answer 409 with
  `code: "limit_reached"` and a message naming the limit. 404s carry
  `list_not_found` or `todo_not_found`. The client treats a 404 on a delete as
  success.
- Use cases return an outcome (`Applied`, `Stale(current)`, `LimitReached`)
  rather than stretching `DomainError`; the handler maps it to a status.
- `PATCH` is added to the CORS methods.
- No `require_fresh_session()`: like other ordinary routes, these trust the
  access token until it expires.
- **Validation:** `ListName` and `TodoTitle` in the domain trim, count code
  points, and refuse control characters and U+2028/U+2029. The rule counts
  after trimming, which `#[validate(length)]` can't express, and a `maxLength`
  in the schema would call accepted padded text invalid. So the schema states
  a pattern (one line, not blank) and the limit is in its description, and
  `positive_data_acceptance` is off in `apifuzz/schemathesis.toml` for the
  operations taking a name or title, with that reason. The UI counts trimmed
  code points and never uses `maxlength`, which counts UTF-16 units. The
  `Debug` output of both types hides the text.

### D6. Order is a gapped integer, renumbered when a gap runs out (domain, persistence-postgres)

Open todos have positions 2^20 apart. A move takes the midpoint of its
neighbours; if there is none, the list's open todos are renumbered in one
statement, under the list lock. The midpoint rule is a pure domain function, so
it is property-tested.

A move sends up to 20 open todo IDs above the target position and 20 below,
nearest first. The server ignores the moved todo's own ID and any that aren't
open in this list, including foreign IDs, and then places the todo directly below the first remaining ID in
`above`, else directly above the first remaining one in `below`, else at the
bottom. An `above` sent empty means the top; one whose IDs are all ignored does
not. So a client whose only neighbours above were never saved sends the moved
todo's own ID there. Moving a done todo answers `stale`. A todo already in its
slot isn't rewritten, so repeats don't use up the gap.

The answer holds IDs only, since full rows would be up to 500 KB per move. If
its set of IDs differs from the open todos the tab knows, something changed
elsewhere, and the tab refetches the list.

- **Rejected: fractional string keys.** They grow without bound, and their main
  benefit, ordering without a lock, isn't needed.
- **Rejected: target index.** It means a different place once the list changes.
- **Rejected: renumbering on every move.** Up to 1,000 row writes per drag.
- **Accepted:** if all 40 neighbours disappear, the todo goes to the bottom.

### D7. The client is confirmed state plus a queue (web, `lib/lists/`)

A module-level runes store holds what the server last confirmed and a queue of
the Member's unsaved operations. The screen is the queue applied to the
confirmed state, so undoing a failed save is removing it from the queue.

- **One queue of saves per tab**, one at a time, in the order made. An
  operation on an item still being added waits for its ID, moves included; a
  neighbour that still has no ID when a move is sent is left out.
- **Loads run beside the queue**, so a slow save doesn't delay opening another
  list. A newer load of a list replaces an older one, whose answer is dropped
  and not repeated. A save to the list sent, finished or in flight since the load
  started also makes its answer stale: it is dropped, and the list is loaded
  again once no save to it is queued. `GET /lists` follows the same rule across
  all lists. Loads and saves are counted apart: with one counter, as first
  built, two overlapping loads repeated each other without end.
- **Refreshing** never hides confirmed todos. They stay on screen with the queue
  applied, and a failed refresh is shown above them with a retry.
- **One name per item:** a new todo's client key and its ID name the same item,
  so an edit queued before the ID arrived and one queued after it depend on each
  other, for versions and for dropping.
- **Order:** open todos, then done ones newest first, as the server orders them.
  An answer moves a todo only if it was ticked or unticked.
- **Counts:** a loaded list's open count comes from its todos with the queue
  applied, unless `GET /lists` answered later and nothing is queued for it;
  other lists use `GET /lists`.
- **Account check:** cookies are shared by all tabs, so a save is sent only if
  the account last confirmed in any tab (an ID in `localStorage`) is this tab's.
  If not, `/auth/me` decides first; the screen doesn't wait. Another account
  clears the store. A switch while a request is already on its way isn't caught.
- **Session ended:** the client tells the store before it redirects
  (`sessionEnded()` in `lib/api/account-changed.ts`), which drops the queue, so
  leaving for the sign-in page doesn't ask to confirm. The save isn't repeated
  and no message is shown.
- **Versions:** an operation stores the version on screen when the Member
  acted. When a save moves a todo from version n to n + 1, later queued
  operations on that todo at n move to n + 1. Nothing else changes a stored
  version: not a refetch, not a success whose version isn't n + 1, and not a
  success that asked for what version n already held. That last one changed
  nothing, so n + 1 is another tab's change; the prototype model (4.4) found that
  it advanced a queued deletion onto a change the Member hadn't seen.
- **Renames** take their version when the form opens, not when it is saved: the
  form hides what a load brings in meanwhile. Until submitted, a started rename
  follows this tab's own saves like a queued edit, and nothing else.
- **Moves** have no version, and go to the place the server's rule would give on
  screen. The answered order is applied beneath what is queued.
- **409 or 404:** the operation and later queued ones on the same item are
  dropped, the current version is shown (or the item removed), and the Member
  sees one message. Dropped text stays available to submit again: it goes back
  into an empty input, else it is listed below with "Try again". No text replaces
  another, or what is being typed.
- **400 or limit:** the same, without changing the confirmed item.
- **Network failure, 15-second timeout or 5xx:** the outcome is unknown. The
  save is repeated once; if that fails too, it is dropped as above and the list
  is refetched: at once, and again on `online`, focus and the next save the
  server answers, until a refetch succeeds. A save is never repeated after the
  store was cleared, when the cookie may belong to another Member.
- **List deletion** waits for the server, because it is permanent.
- **Out-of-order loads:** an answer is applied only to the list it names.
- **Leaving:** the store survives in-site moves. `beforeunload` asks to confirm
  while the queue isn't empty; it is listened for only then, since a permanent
  listener keeps the page out of the back/forward cache. The store is in memory only, and is cleared on
  sign-out and when the account changes, also when the background `/auth/me`
  check is what finds the other account; open panels then load again.
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
- The rewrite and the template match by prefix, so a malformed address gets the
  same shell and the same page name.
- **Rejected: `/dashboard/lists?id=…`.** No rewrite needed, but the address is
  worth being a path, and query strings are stripped from telemetry by design.
- **Slice 1 check:** `astro preview` does not run the plugin (Astro replaces
  the user plugin list), so `pnpm preview` runs the Caddyfile instead. The
  production and test rewrites are then the same file.

### D9. Keeping content and list IDs out of telemetry (web, application)

- **Page template:** anything under `/dashboard/lists/` becomes
  `/dashboard/lists/[id]`, malformed IDs included. In the browser this happens
  in `pageOf`, which every kind of report uses; on the server in
  `page_template`, which is an exact match today.
- **Scrubbing:** a new rule, in both copies and the shared fixture, replaces
  the rest of the path after `/dashboard/lists/` (keeping a `:line:column`), and
  any UUID after `/lists/` or `/todos/` in an API path, whatever follows it, in a
  message, stack or source.
- **API failures** pass `route`, so reports hold the route template.
- **Analytics:** `analytics.js` defines a window function before the tracker
  loads and names it in `data-before-send` (the pinned Umami, 3.5.0, supports
  it). It rewrites the page and referrer and pins the title to "Lists".
- **Edge access logs** keep list IDs, in the address and `Referer`. Accepted:
  an ID gives no access, only the operator reads these logs, they already hold
  client addresses, and they expire in 30 days. The specs bar list IDs from
  browser reports and analytics only; names and titles are never in an address.
- **Cloudflare Web Analytics:** the script policy allows the beacon Cloudflare
  can inject, which would record real list addresses. It must be off for the
  site; that is a first-deploy check.
- **Backend:** spans and logs carry list, todo and user IDs only. `skip_all`
  everywhere, and the `Debug` rule in D5.
- **Postgres logs:** errors can log row values in their DETAIL line. Validation
  and locking mean the constraints shouldn't fire; `log_error_verbosity=terse`
  is set in the compose files and the Kubernetes cluster as a second net.
- A test writes marker strings through every route, including refused and
  failed saves, and searches the captured logs and spans for them.

### D10. Export and erasure (application, persistence-postgres)

- **Purge:** `anonymize_account` deletes the Member's lists in its existing
  transaction, so a failure leaves the account due and the next run retries.
- **Sweep:** each purge run first deletes lists whose account is already
  deleted, before `run_once` returns early when no account is due. Without it, a purge by the previous release, during a roll or after
  a rollback, would leave lists behind for good.
- **Export:** `lists` is added to the export response, with creation and
  completion times. The worst case (100,000 titles of 500 characters) is about
  200 MB, too much to build in memory on any deploy, so from slice 1 the body is
  streamed one list at a time. Each list is consistent in itself; the export as
  a whole isn't one snapshot. A failure part-way aborts the response, so the
  browser reports a failed download rather than saving a partial file. A list
  deleted since the export started is left out, not a failure. The
  audit event is recorded before the first byte.

### D11. No feature flag

There is no production deploy, and slice 1 is usable on its own, though
reachable only by address. Slice 2 adds the Lists navigation entry, which is
what makes the feature visible.

### D12. Threat model

A new property, P-35 (the next free number), covers the `security` delta. Its checks are route tests
that try every list operation as another Member, an Admin, a Super Admin, an
unverified Member and a Visitor, comparing each 404 with that of an unknown ID.
The existing route-guard walk covers the Visitor case for future routes.

### D13. Formal coverage of lists (domain, persistence-postgres, web)

- **Scope:** every OT-013 guarantee is classified as a proof, a model with
  conformance replay, or a test with a written reason no proof or model applies.
  "Everything" means every guarantee, not every line of UI, SQL text or
  third-party code.
- **Proofs cover code that runs.** SQL bumps versions (`version = version + 1`),
  and nothing in production calls `List::rename`, `Todo::mark_done`,
  `Todo::mark_not_done` or `Todo::rename`. They are removed, and the version rule
  is checked by the server model and its replay, not by Kani.
- **Domain proofs (Kani):** ordering and validation over integer inputs.
  - Kani can't get through `jiff` or OS randomness, so harnesses use integer
    kernels and no `Timestamp` or `Uuid::new_v4`. The open-or-done rule is
    extracted as a kernel over the two columns' presence, used by
    `from_persistence`.
  - The midpoint and renumbering kernels are written and proved in slice 4, before
    the repository uses them; the other proofs come in slice 6.
  - The midpoint kernel covers all `i64` neighbours, with no overflow in the
    midpoint or in moves above the first. At either end of `i64` it asks for
    renumbering. `append_position` is strictly increasing unless it saturates;
    at saturation the caller must renumber, or the bound is stated with why
    it can't be reached.
    _Slice 6:_ the bound is stated and proved instead (`todo.rs`): no write
    raises a list's highest position by more than one gap, and renumbering
    stays under 65,537 gaps, so saturating takes about 2^43 writes to one list.
    Were it reached, the unique constraint on positions refuses the write.
  - CBMC can't unwind hundreds of symbolic characters or `String` allocations, so
    `normalize_single_line` uses an allocation-free kernel, proved over up to 6
    symbolic `char`s with `max <= 4`. A proptest ties it to the real function
    at 100 and 500 characters.
  - Renumbering is proved for up to 5 positions; a proptest covers 1,000.
- **Server model (Quint):** `list_writes.qnt`, two Members, two writers in flight.
  - `MAX_LISTS = 2` and `MAX_TODOS = 3`, with 4 list IDs and 5 todo IDs: one
    Member can exceed either limit while another Member's row exists.
    An ID universe no larger than a limit would make that invariant vacuous.
  - `init` chooses among populated states that meet the table constraints: at and
    just below each limit, lists of three open todos, done todos, and adjacent
    positions. Starting empty, 8 steps reach neither a full list nor two
    concurrent moves. Positions use the real `POSITION_GAP`, and renumbering is
    reached from adjacent seeds, not by exhausting a 2^20 gap.
  - Each write is `begin` (take the lock, then read) and `commit`. `create_list`
    locks the user and is the only write that checks `deleted_at`; every other
    write locks its list. Purge takes the user, then its lists. The D10 sweep
    deletes lists of purged accounts. `find_list` is two reads, since under READ
    COMMITTED the todos may be newer than the list row.
  - ~~Apalache checks 8 steps; 10,000 random traces check 30.~~ _Revised by
    7.1 (2026-11-26), after the prototype:_ each request is `send`, `begin`
    and `commit`, so two interleaved requests take 6 steps. Apalache took 55 s
    at 2 steps and 226 s at 3, about four times per step, so 6 steps would take
    hours and 8 days. As 8.1 decided for the queue, random simulation is the
    gate: every invariant over 100,000 traces of 30 steps (about 40 s), and
    each broken variant must fail within 200,000 (`no_lock`, the slowest, after
    about 50,000). Apalache checks 3 steps, every single request from every
    initial state (about 4 minutes). Races rest on the simulation and on the
    overlapping-transaction tests (7.5).
- **Invariants are scoped to what the design promises:**
  - Version rules cover renames, marks and deletes. A state already stored
    succeeds at any version and changes nothing. Creations start at 1, and moves
    neither check nor change a version.
  - A repeated create or add stores one row while that row still exists. Accepted
    exceptions (Risks): a repeat after another tab deleted the item recreates it,
    and a repeated move may undo a later one, leaving one order.
- **Locks need concurrent tests.** Replay calls each repository method once, at
  `commit`, so it can't notice a missing `FOR UPDATE`. Every lock-dependent rule
  also names an `#[sqlx::test]` with overlapping transactions, shown to fail with
  the lock removed. Interleaved `find_list` reads are modelled but not replayed.
- **Client model (Quint):** two smaller models instead of one:
  `list_queue.qnt` (queue, versions, drafts, retry, 404 and stale answers, moves,
  reset and account switch) and `list_loads.qnt` (load and save counters, dropped
  loads, reloads, recovery).
  - A request has separate `send`, `apply` (at the server) and `answer` steps, and
    `apply` may come after a timeout, a retry or a refetch. The replay transport
    has the same three steps.
  - A reset drops the queue, and no request starts for the old epoch. A request
    already on its way isn't recalled (D7).
  - Convergence is a safety property: if drained, recovered and reloaded, with
    no request in flight, each tab equals the server. That queues do drain is a
    liveness claim, covered by the seeded tests under fair answers.
- **Memory server:** client replay trusts `MemoryListsServer`, so the server
  model's traces are replayed through it as well as Postgres, for the operations
  it implements. Ownership, purge and sweep steps are Postgres-only.
- **Bounds are measured first.** Each model starts with a prototype recording
  state-space size and runtime. Its bounds are fixed from that and written
  next to it, never raised or lowered silently. `list_queue.qnt` is first
  prototyped without moves in slice 4, before the client queue gains them, for at
  most a day: most slice 3 review findings were in that queue.
- **Prototype result (slice 4, 2026-11-26).** `models/list_queue.qnt`: two
  tabs, one todo (also run with two), queues of 2, 5 requests, 3 titles; mark,
  rename by draft, delete, single retry, late applies, 404 and stale answers,
  loads. No additions, moves, resets or account switches yet.
  - **Found a bug** in the code as it stood (D7, now fixed with a regression
    test): a rename asking for the title version n already held succeeded at
    n + 1, another tab's tick, and advanced a queued deletion onto it. The random
    simulation found it within 100,000 traces of 25 steps; the trace is longer
    than Apalache can check.
  - **Apalache, alone on this machine:** 4 steps 30 s, 5 steps 126 s, 6 steps
    616 s, about five times per step; 8 steps would take hours. At 6 steps
    `refetch_advances_queue` and `refetch_advances_draft` give counterexamples
    in 701 s and 580 s. State-space size isn't reported by Apalache and wasn't
    measured.
  - **So 8.1 changes:** Apalache can't be the main check for the queue at
    useful depths. Plan: `quint run` (random simulation, 25 to 30 steps, a fixed
    number of samples) as the gate for the queue models, with Apalache at 5 steps
    as a fast bounded check; each broken variant must fail the simulation within
    its sample count. To be decided in 8.1, with the model kept small.
- **Coverage gate:** `docs/formal-coverage.yaml` is keyed by capability,
  requirement and scenario, plus P-35. `just formal-coverage` reads the spec
  deltas while the change is active and `openspec/specs/` once it is archived.
  The manifest lists the requirements in scope, so other features in merged specs
  are not demanded.
  - A check must exist and run: Rust tests aren't `#[ignore]`d (except the
    conformance suite `just models` runs), web tests aren't skipped or todo, Kani
    functions carry `#[kani::proof]`, and Quint invariants and broken variants are
    invoked by `scripts/models.sh`.
  - An entry may be `pending: <task>` while that task is unticked. A pending entry
    for a ticked task, or any once the feature is done (`--final`, then the default
    from task 9.1), fails.
- **Not formalized:** rendering, layout, CSS, SQL text, HTTP wiring, telemetry
  content filtering and third-party behaviour. Their requirements state their
  tests and why a proof or model does not apply.

## Risks / Trade-offs

- [A formal check drifts from the code it describes] → Model steps are
  replayed through the real repository or engine (D13 names the exceptions), and `just formal-coverage`
  fails when a named check disappears. Update the model and replay in the same
  commit as the code.
- [Kani or Quint runtime grows past the gate] → Proofs stay on integer cores,
  each model is prototyped before its bounds are fixed, and the client is two
  models. Each runs as its own job. Record runtimes.
- [The client model freezes the wrong behaviour] → Freeze D7 after the review,
  model only the public engine API, and prove each invariant can fail before
  trusting it.
- [The coverage gate becomes a vacuous checklist] → Every guarded invariant
  names a broken variant that must produce a counterexample.
- [Repeating a lost add after the todo was deleted in another tab recreates it]
  → Only within the single immediate repeat; the Member can delete it again.
- [A request the browser gave up on is applied after the refetch] → The next
  edit gets a 409 with the current version, so nothing is overwritten.
- [Repeating a lost move can undo a later move from another tab] → Both end on
  one order, which is what the specs promise for moves.
- [One queue per tab means a slow save delays the rest] → Requests are small,
  and the timeout and single repeat bound it at about 30 seconds.
- [A list write lock serialises a shared list's editors later] → Transactions
  are a few short statements. Measured in slice 2 (2026-11-25, capped stack,
  10 iterations/s for 3 minutes, worst of 5 runs of each scenario, no errors):
  p95 / p99 in ms.
  - `lists`: GET lists 1.44 / 1.92, POST list 3.58 / 7.26, GET list 1.19 / 1.75,
    POST todo 2.82 / 7.31, PATCH todo 2.55 / 6.56.
  - `lists-large` (five lists of 1,000 todos, each open in several tabs): GET
    list 4.12 / 5.11, PATCH todo 7.29 / 7.80. All budgets are at their floors of
    25 / 50.
  - Slice 4 (2026-11-26, same stack): `lists-large` also moves a todo each
    iteration, directly below the first open todo, so about one move in 20
    renumbers a list of 1,000. Five runs: move p95 7.1 to 8.3, p99 15 to 16,
    max 95 ms. Four runs met every budget; one had PATCH todo p99 at 50 ms (max
    77), against 3.2 to 4.2 ms in the others. Sampling during two more runs
    showed 0 to 17,000 dead todo rows (5,079 live), autovacuum about once a
    minute, `todos` indexes at 34 MB against a 1.6 MB table, and no lock waits
    in 3-second samples.
  - The slow tail is the developer machine's WAL flush, not the list code
    (2026-11-26, five more runs with every request timed by k6 and Postgres
    logging statements over 20 ms, lock waits over 10 ms, checkpoints and
    autovacuums). All five met every budget: PATCH todo p99 3.5 to 6.6 ms, max
    74. Ten of 27,000 requests exceeded 25 ms, singly, with the requests beside
    them fast, and each slow tick matched a slow `COMMIT` to the millisecond
    (74 ms tick, 72 ms `COMMIT`). No statement was slow, no lock wait was
    logged, and the slow commits fell outside autovacuums and mostly outside
    checkpoints. The database volume is on btrfs (zstd, copy-on-write) inside
    LUKS, known for occasional fsync stalls; the failing run would be a worse
    patch of them. Index bloat is reusable space: after autovacuum about 95% of
    the index pages are free, and no query ran slow. Renumbering adds WAL but
    does not create the stalls: ticks reached 13 to 33 ms before moves existed.
    Accepted as environmental. If it recurs, put the load database on a
    no-copy-on-write directory rather than turning off `synchronous_commit`,
    which would hide what a commit costs.
- [The largest export exhausts memory] → Streamed (D10). Measured in slice 2,
  100 lists of 1,000 todos with 500-character titles, always complete:
  62.5 MB in 0.5 s with ASCII titles, and 212.5 MB in 1.0 s with four-byte
  characters. In the four-byte case backend memory peaked at 155 MiB since
  start, sign-in included, under the 1 GB cap, also with a reader that took
  131 s.
- [1,000 rows make drag or keyboard moves slow] → `content-visibility: auto`,
  and an Interaction to Next Paint check on a full list in the ordering slice.
  Measured in slice 5 (2026-11-26, this machine):
  - `tests/lists.spec.ts` "a full list" opens 999 todos, adds the 1,000th, then
    marks, renames and moves by keyboard and by drag, and fails if any
    interaction's latency, as the browser measures it for INP, reaches 200 ms.
    It runs in `just e2e`, so the budget holds on the CI runner too. Three runs:
    every interaction 16 to 48 ms. Without `content-visibility`: 16 to 64 ms,
    the moves slowest, so it helps most where layout is the cost.
  - A first run reported 288 ms for opening a rename. A CPU profile showed
    Playwright's label lookup (`getByLabel`) computing labels for all 1,000
    rows in the page, during the click's frame; the panel itself took about
    4 ms. The flow therefore finds fields by ID while it measures.
  - `ListsPanel.bench.ts` (Vitest, `just bench-web`, no Tailwind, so without
    `content-visibility`): opening 1,000 todos 113 ms mean (p99 189); each
    change and its undo, both saved, 66 to 73 ms mean (p99 88 to 95).
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

**First deploy (slice 1):** boot applies the migration, then `just smoke <url>`,
then create a list by address (`/dashboard/lists`), add and tick a todo, and
check the export. Confirm Cloudflare Web Analytics is off: it would inject a
second page-view beacon that D9's rewrite does not cover.

## Open Questions

- _Resolved (client trace replay):_ `scripts/models.sh` exports the client
  models' ITF traces into `MODEL_TRACES_DIR` (`backend/target/model-traces`) and
  runs a Node-mode Vitest replay through `ListsEngine` and `MemoryListsServer`.
  No trace files are committed and `just test-web` is untouched.
