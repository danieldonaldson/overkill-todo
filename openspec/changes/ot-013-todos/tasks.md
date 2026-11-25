# Tasks

All slices are planned up front, by choice, rather than after the first one
ships. Tasks 1.15 and 2.6 revise the later slices with what was learned.

## 1. Slice 1 (first slice): create a list, open it, add and tick todos, with conflict-safe saves

Retires the riskiest unknown: optimistic edits that don't conflict with
themselves, never overwrite another tab, and undo cleanly. Lists are
reachable only by address until slice 2 adds the navigation entry. Renaming,
deleting and reordering come later; the tables already allow for them.

Delivers these scenarios:

- **lists:** No lists yet; Create a list; Count of open todos; Reload or
  bookmark; Address does not open a list (except a deleted list); Shared
  address; Empty list; Add a todo; Mark done; Mark not done; Blank; At the
  length limit; Too long; Control characters sent to the server; Pasted lines;
  Markup in a title; List limit; Todo limit; Concurrent additions at the limit;
  Marked done in both tabs; No version given (marking); Answer lost; Same title
  on purpose; Export; Export with no lists; Deletion pending, then cancelled;
  Account purged; Purge fails part-way; Largest
  export (measured in slice 2).
- **dashboard:** Move between lists; Slow answers arrive out of order; Visitor
  opens a list address; Slow server; Server or database unavailable; Another
  account signs in; Change appears at once; Edit a todo that is still being
  added (marking); Addition fails; One of several
  saves fails; An earlier change to the same todo fails; Messages are announced.
- **security:** all except Positioning next to another Member's todo.
- **observability, analytics:** all, for the actions this slice has.

- [x] 1.1 Check list addresses first (D8): `pages/dashboard/lists/index.astro` and `[id].astro` (one placeholder shell), the rewrite in `apps/web/Caddyfile`, and the Vite plugin for dev and preview. Add both templates to `PAGE_TEMPLATES`. Verify with a Playwright spec under `astro preview` that a list address survives a reload, an in-site move and back, and that `tests/caddy-headers.spec.ts` and `tests/csp.spec.ts` pass. If the plugin doesn't work in preview, switch to the fallback in D8 and record it there. The rewrite matches by prefix, so a malformed address gets the shell too. The spec also covers "Visitor opens a list address".
- [x] 1.2 Migration: `lists` and `todos` as in D2. `#[sqlx::test]` cases: a todo can't be both open and done or neither, positions are unique per list, one statement can renumber a list, deleting a list deletes its todos. Rollback plan: redeploy the previous digest; the tables stay unused (Migration Plan).
- [x] 1.3 Domain: `ListId`, `TodoId`, `ListName`, `TodoTitle`, the two entities with `version`, the limits, and the append position (D5, D6). Property tests: trimming, code point counts with emoji and combining marks, control characters and U+2028/U+2029 refused, and `Debug` never shows the text.
- [x] 1.4 Application: the list repository port and use cases for listing, creating a list, opening one, adding a todo and marking it, returning `Applied`, `Stale(current)` or `LimitReached` (D5). Mock tests cover every branch.
- [x] 1.5 Persistence: the repository, every write locking its list (or the `users` row) first (D3, D4). `#[sqlx::test]` cases: a repeated key returns the existing row, also at the limit; concurrent additions at 999 todos and 99 lists; stale mark refused with the current row; a mark already in place succeeds with the version unchanged; marking done and not done moves the todo as specified; another Member's list and key are invisible; no list can be created for a deleted account. Run `just db-prepare` and commit `backend/.sqlx/`. Also assert lists oldest first, done todos most recently completed first, and a deleted account refused (401 at the API).
- [x] 1.6 API: `GET` and `POST /lists`, `GET /lists/{id}`, `POST /lists/{id}/todos`, `PATCH /lists/{id}/todos/{todo_id}` (`done` only for now), the 409 bodies and the 404 codes, and `PATCH` in CORS (D5). Integration tests for each status, including 400 without a version. Run `just api-types` and commit `backend/openapi.json` and `schema.d.ts`. Verify with `just fuzz-api`; any exclusion goes in `apifuzz/schemathesis.toml` with its reason. The todo `PATCH` body is the D5 enum, with only its `done` form for now; no answer carries a position. Turn `positive_data_acceptance` off for `POST /lists` and `POST /lists/{id}/todos`, with the D5 reason. Test a list creation with a purged account's unexpired token: 401.
- [x] 1.7 Threat model: add P-35 with its route tests as another Member, Admin, Super Admin, unverified Member and Visitor, and without a CSRF token, comparing each 404 with an unknown ID's (D12). Verify with `just threat-model-check`.
- [x] 1.8 Erasure: `anonymize_account` deletes lists in its transaction, and each purge run sweeps lists of accounts already deleted (D10). `#[sqlx::test]` cases: purged, another Member untouched, cancelled deletion keeps everything, an injected failure leaves the account due, and the sweep clears lists left by an older release. The sweep runs before the early return: test it with no account due.
- [x] 1.9 Export: lists and their todos, in order, with creation and completion times, streamed one list at a time; a failure part-way aborts the response (D10). Integration tests: open and done todos, no lists, only the caller's own, an injected failure gives an incomplete response, the audit event comes first, and the 30-second request timeout doesn't cut off a slow body. Web: an incomplete export shows an error and saves no file (Vitest). Run `just api-types`.
- [x] 1.10 Client store: confirmed state plus queue in `apps/web/src/lib/lists/`, for adding and marking (D7). Unit tests: undo of one failed save among several, dependent edits dropped with one message, stale answer, 404, limit, unknown outcome then refetch, version rebasing only on n + 1, cleared on sign-out. Also: loads beside the queue with the per-list counter, a stale load dropped and repeated, marking a todo still being added, counts for loaded and other lists, and cleared on an account change.
- [x] 1.11 Hypothesis test: two copies of the queue against an in-memory server following D4, with seeded delays, failures, lost answers and refetches. Verify it passes for 10,000 seeds, and that it fails when a refetch is allowed to advance a queued edit; keep that broken variant as a test that expects failure. A bounded seed count runs in `just test-web`. After both queues drain, refetch once in each tab before comparing with the server (Hypothesis). Include marking a todo still being added.
- [x] 1.12 Basic web UI: the list picker and panel (`client:load`, `TelemetryBoundary`, name added to `client_islands.json` and `ISLAND_TEMPLATES`), create, add, mark, the "Completed" group, placeholders, empty, failed and "list not found" states, the address replaced once a new list is saved (D3), and the toaster mounted in the persisted header. Vitest browser tests cover each state, literal markup, pasted lines and announced messages. The address is replaced with `history.replaceState` (D3): test that the header and queue survive it and that back still works. Count trimmed code points, with no `maxlength`: test 500 emoji.
- [x] 1.13 Telemetry and analytics privacy (D9): the prefix rule in `pageOf` and `page_template`, tested with a UUID, a malformed ID and a trailing slash; the scrub rule in both copies, with dashboard and API path cases in `client_scrub_cases.json`; `route` on API calls; and the Umami before-send function. Set `log_error_verbosity=terse` in the compose files. Verify with a backend marker-string test over every new route, refused and failed saves included, by extending `tests/analytics.spec.ts` (vendored 3.5.0 tracker) to check the rewritten page, referrer and title, and by extending `tests/browser-errors.spec.ts` to look for list IDs, names and titles.
- [x] 1.14 Flows: Playwright with the API mocked for create, open, add, mark and one failed save; a two-tab flow against the real backend in `tests/integration/` that marks the same todo in both tabs and loses an answer. Verify with `just e2e` and the integration run. Include "Shared address" and slow answers arriving out of order.
- [x] 1.15 Record the result of 1.11 in design.md Hypothesis and revise the decisions and later slices it changed. Get `just ci` green. Note for the first deploy: the migration runs at boot, then `just smoke <url>`, then create a list by address, add and mark a todo, and check the export. Also check at the first deploy that Cloudflare Web Analytics is off (D9).

## 2. Slice 2: lists in the navigation, remaining failure states, load and alerting

Split off from the first slice to keep it within about 5 days.

Delivers these scenarios (dashboard): List panel crashes; Open Lists from the
navigation; Lists load but todos fail; Signed out part-way; Moving to another
dashboard page while saving; Closing the tab while saving.

- [ ] 2.1 Remaining states: a crash contained to the list panel, todos failing while the picker works, session loss while saving, a save failing after an in-site move, and `beforeunload` (D7). Vitest browser tests for each.
- [ ] 2.2 Flows: Playwright with the API mocked for slow answers out of order, moving and closing while saving, and an expired session. Check at 360 px without horizontal scrolling.
- [ ] 2.3 Performance: a load scenario that opens a list, adds and marks todos, and a run against a list of 1,000. Add its budget and baseline, and record the numbers in design.md Risks. Also export 100 lists of 1,000 todos with 500-character titles on the capped stack: it must be complete, with backend memory within the cap. Record time and peak memory.
- [ ] 2.4 Alerting: confirm that failed list saves are covered by the existing 5xx alert and that a failure is found in the logs by its trace ID, without the title. Verify by forcing one locally.
- [ ] 2.5 Add the Lists navigation entry, with `DashboardNav` tests, and update README and `backend/ARCHITECTURE.md`. Get `just ci` green. Note for the first deploy: `just smoke <url>`, then open Lists from the navigation.
- [ ] 2.6 Revise design.md with what both slices showed, and revise slices 3 to 5 below to match. Verify with `openspec validate ot-013-todos --strict`.

## 3. Slice 3: rename and delete todos and lists, with their conflicts

Delivers these scenarios:

- **lists:** Rename a list; Confirm deletion; Cancel deletion; Todo added in
  another tab before confirming; Address does not open a list (deleted list);
  Rename a todo; Delete a todo; Abandon a rename; Room again after deleting;
  Renamed in two tabs; Renamed in one tab, marked done in another; Deleting a
  todo renamed elsewhere; List renamed in two tabs; Different todos in two
  tabs; No version given (renaming and deleting); Todo deleted elsewhere
  (renamed or marked); List deleted elsewhere; Deleted in both tabs.
- **dashboard:** Rapid edits in one tab; Edit a todo that is still being added
  (renaming and deleting);
  Save fails; Unknown outcome.
- **security, observability, analytics:** their scenarios again, for the new
  actions.

- [ ] 3.1 Application and persistence: rename a list or todo, delete a todo, and delete a list by `version` and `todo_count`, all under the list lock (D4, D5). Mock tests for every outcome. `#[sqlx::test]` cases: stale rename and delete refused with the current row, a rename already in place succeeds with the version unchanged, different todos don't conflict, list deletion refused when the name or count changed and serialised with a concurrent add, and room again at both limits. Run `just db-prepare` and commit `backend/.sqlx/`. The refusal of a list deletion carries the current name, version and count.
- [ ] 3.2 API: `PATCH` and `DELETE /lists/{id}`, `DELETE /lists/{id}/todos/{todo_id}`, and `title` in the todo `PATCH`, with exactly one of `title` and `done` (D5). Integration tests for each status, including neither or both fields and a missing version. Run `just api-types`; verify with `just fuzz-api`. Turn `positive_data_acceptance` off for both `PATCH` operations, with the D5 reason.
- [ ] 3.3 Extend the P-35 route tests and the marker-string telemetry test to the new routes. Verify with `just threat-model-check` and `just test`.
- [ ] 3.4 Client store: rename and delete operations, edits queued on an item still being added, dependent edits dropped with one message, 404 removes the item, 404 on a delete counts as success, and dropped text kept to submit again (D7). Extend the two-tab random test with renames and deletions. Verify 10,000 seeds pass and the broken variant still fails.
- [ ] 3.5 Web UI: rename in place (cancel with Escape), delete a todo, the list deletion confirmation with its count and second confirmation, and the "changed elsewhere" and "deleted elsewhere" notes. Deleting a list is not optimistic. Vitest browser tests for each, including focus after a deletion. A list can be deleted only while open.
- [ ] 3.6 Flows: Playwright with the API mocked for rename, failed rename, lost answer and list deletion; two-tab flows in `tests/integration/` for each conflict scenario above.
- [ ] 3.7 Update README if needed, and get `just ci` green. Note for the first deploy: `just smoke <url>`, then rename and delete a todo and a list.

## 4. Slice 4: reorder open todos

Delivers these scenarios:

- **lists:** Drag to a new position; Move with the keyboard; Cancelled drag;
  Many moves; Moved in one tab, renamed in another; Todo deleted elsewhere
  (moved); Different todos moved at once; Same todo moved at once; Neighbour
  gone; Moved todo was completed elsewhere.
- **dashboard:** Move fails.
- **security:** Positioning next to another Member's todo.

- [ ] 4.1 Domain: the midpoint and renumbering rules as pure functions (D6). Property tests: a move lands strictly between its neighbours or asks for renumbering, and renumbering keeps the order.
- [ ] 4.2 Application and persistence: the move use case and repository method, with `above` and `below` resolved under the list lock. Mock tests per outcome. `#[sqlx::test]` cases: top, middle and bottom; 1,000 moves between the same two todos; concurrent moves, adds and completions leave every todo once; neighbours deleted, done or foreign; a done or deleted todo; the version unchanged. Run `just db-prepare` and commit `backend/.sqlx/`. The todo's own ID in `above` or `below` is ignored.
- [ ] 4.3 API: `POST /lists/{id}/todos/{todo_id}/move`, with at most 20 IDs each way. Integration tests for each status, and a foreign neighbour answered exactly as an unknown one (P-35). Run `just api-types`; verify with `just fuzz-api` and `just threat-model-check`.
- [ ] 4.4 Client store: move operations that apply the returned order beneath pending operations (D7). Extend the two-tab random test with moves: no todo lost or duplicated, and one order once both queues drain. Verify 10,000 seeds pass. An answer whose IDs differ from the known open todos triggers a refetch, and a move of a todo still being added is sent after its creation; test both.
- [ ] 4.5 Web UI: a drag handle for mouse and touch with `touch-action: none` on the handle alone, cancel with Escape, and keyboard moves that keep focus and announce the position. Vitest browser tests for each, and that done todos have no handle.
- [ ] 4.6 Flows: Playwright with the API mocked for drag, keyboard, cancelled drag and failed move; a two-tab flow in `tests/integration/` for concurrent moves.
- [ ] 4.7 Get `just ci` green. Note for the first deploy: `just smoke <url>`, then reorder a list and reload it.

## 5. Slice 5: phones and full lists

Delivers these scenarios:

- **lists:** A full list stays usable.
- **dashboard:** Phone; Touch.

- [ ] 5.1 Phone layout: the picker as a drawer that traps focus, closes on choosing a list and returns focus. Vitest browser and Playwright tests at 360 px without horizontal scrolling.
- [ ] 5.2 Touch: a Playwright test with touch emulation that swipes over todos (the list scrolls, nothing moves) and drags by the handle.
- [ ] 5.3 Full lists: `content-visibility: auto` on rows. Check adding, marking, renaming and reordering on 1,000 todos with a Vitest bench and a Playwright flow, with interactions under 200 ms on the CI runner. Record the numbers in design.md Risks.
- [ ] 5.4 Get `just ci` green. Note for the first deploy: `just smoke <url>`, then open a list on a phone.

## 6. Feature done

- [ ] 6.1 Every scenario in the spec deltas is delivered by a slice above and has a test; list any exception with its reason. Verify by walking the specs against the tests.
- [ ] 6.2 README, `backend/ARCHITECTURE.md` and the threat model match what was built, and the first-deploy checks are recorded. There is no flag to remove (D11). Verify with `just ci`.
- [ ] 6.3 Archive the change with `/opsx:archive`, and verify with `openspec validate --specs --strict`.
