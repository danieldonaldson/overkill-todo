# Tasks

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
  Account purged; Purge fails part-way.
- **dashboard:** Move between lists; Slow answers arrive out of order; Visitor
  opens a list address; Slow server; Server or database unavailable; Another
  account signs in; Change appears at once; Addition fails; One of several
  saves fails; An earlier change to the same todo fails; Messages are announced.
- **security:** all except Positioning next to another Member's todo.
- **observability, analytics:** all, for the actions this slice has.

- [ ] 1.1 Check list addresses first (D8): `pages/dashboard/lists/index.astro` and `[id].astro` (one placeholder shell), the rewrite in `apps/web/Caddyfile`, and the Vite plugin for dev and preview. Add both templates to `PAGE_TEMPLATES`. Verify with a Playwright spec under `astro preview` that a list address survives a reload, an in-site move and back, and that `tests/caddy-headers.spec.ts` and `tests/csp.spec.ts` pass. If the plugin doesn't work in preview, switch to the fallback in D8 and record it there.
- [ ] 1.2 Migration: `lists` and `todos` as in D2. `#[sqlx::test]` cases: a todo can't be both open and done or neither, positions are unique per list, one statement can renumber a list, deleting a list deletes its todos. Rollback plan: redeploy the previous digest; the tables stay unused (Migration Plan).
- [ ] 1.3 Domain: `ListId`, `TodoId`, `ListName`, `TodoTitle`, the two entities with `version`, the limits, and the append position (D5, D6). Property tests: trimming, code point counts with emoji and combining marks, control characters and U+2028/U+2029 refused, and `Debug` never shows the text.
- [ ] 1.4 Application: the list repository port and use cases for listing, creating a list, opening one, adding a todo and marking it, returning `Applied`, `Stale(current)` or `LimitReached` (D5). Mock tests cover every branch.
- [ ] 1.5 Persistence: the repository, every write locking its list (or the `users` row) first (D3, D4). `#[sqlx::test]` cases: a repeated key returns the existing row, also at the limit; concurrent additions at 999 todos and 99 lists; stale mark refused with the current row; a mark already in place succeeds with the version unchanged; marking done and not done moves the todo as specified; another Member's list and key are invisible; no list can be created for a deleted account. Run `just db-prepare` and commit `backend/.sqlx/`.
- [ ] 1.6 API: `GET` and `POST /lists`, `GET /lists/{id}`, `POST /lists/{id}/todos`, `PATCH /lists/{id}/todos/{todo_id}` (`done` only for now), the 409 bodies and the 404 codes, and `PATCH` in CORS (D5). Integration tests for each status, including 400 without a version. Run `just api-types` and commit `backend/openapi.json` and `schema.d.ts`. Verify with `just fuzz-api`; any exclusion goes in `apifuzz/schemathesis.toml` with its reason.
- [ ] 1.7 Threat model: add P-33 with its route tests as another Member, Admin, Super Admin, unverified Member and Visitor, and without a CSRF token, comparing each 404 with an unknown ID's (D12). Verify with `just threat-model-check`.
- [ ] 1.8 Erasure: `anonymize_account` deletes lists in its transaction, and each purge run sweeps lists of accounts already deleted (D10). `#[sqlx::test]` cases: purged, another Member untouched, cancelled deletion keeps everything, an injected failure leaves the account due, and the sweep clears lists left by an older release.
- [ ] 1.9 Export: lists and their todos, in order, in the export response, built in memory for now (D10). Integration tests: open and done todos, no lists, only the caller's own. Run `just api-types`.
- [ ] 1.10 Client store: confirmed state plus queue in `apps/web/src/lib/lists/`, for adding and marking (D7). Unit tests: undo of one failed save among several, dependent edits dropped with one message, stale answer, 404, limit, unknown outcome then refetch, version rebasing only on n + 1, cleared on sign-out.
- [ ] 1.11 Hypothesis test: two copies of the queue against an in-memory server following D4, with seeded delays, failures, lost answers and refetches. Verify it passes for 10,000 seeds, and that it fails when a refetch is allowed to advance a queued edit; keep that broken variant as a test that expects failure. A bounded seed count runs in `just test-web`.
- [ ] 1.12 Basic web UI: the list picker and panel (`client:load`, `TelemetryBoundary`, name added to `client_islands.json` and `ISLAND_TEMPLATES`), create, add, mark, the "Completed" group, placeholders, empty, failed and "list not found" states, the address replaced once a new list is saved (D3), and the toaster mounted in the persisted header. Vitest browser tests cover each state, literal markup, pasted lines and announced messages.
- [ ] 1.13 Telemetry and analytics privacy (D9): page template in browser and server, the scrub rule in both copies with new cases in `client_scrub_cases.json`, `route` on API calls, and the Umami before-send hook or its fallback. Check the hook against the deployed Umami version first. Set `log_error_verbosity=terse` in the compose files. Verify with a backend marker-string test over every new route, refused and failed saves included, and by extending `tests/analytics.spec.ts` and `tests/browser-errors.spec.ts` to look for list IDs, names and titles.
- [ ] 1.14 Flows: Playwright with the API mocked for create, open, add, mark and one failed save; a two-tab flow against the real backend in `tests/integration/` that marks the same todo in both tabs and loses an answer. Verify with `just e2e` and the integration run.
- [ ] 1.15 Record the result of 1.11 in design.md Hypothesis and revise the decisions it changed. Get `just ci` green. Note for the first deploy: the migration runs at boot, then `just smoke <url>`, then create a list by address, add and mark a todo, and check the export.

## 2. Slice 2: lists in the navigation, remaining failure states, load and alerting

Split off from the first slice to keep it within about 5 days, so it is planned
now; slices after it wait for what slice 1 showed.

Delivers these scenarios (dashboard): List panel crashes; Open Lists from the
navigation; Lists load but todos fail; Signed out part-way; Moving to another
dashboard page while saving; Closing the tab while saving.

- [ ] 2.1 Remaining states: a crash contained to the list panel, todos failing while the picker works, session loss while saving, a save failing after an in-site move, and `beforeunload` (D7). Vitest browser tests for each.
- [ ] 2.2 Flows: Playwright with the API mocked for slow answers out of order, moving and closing while saving, and an expired session. Check at 360 px without horizontal scrolling.
- [ ] 2.3 Performance: a load scenario that opens a list, adds and marks todos, and a run against a list of 1,000. Add its budget and baseline, and record the numbers in design.md Risks.
- [ ] 2.4 Alerting: confirm that failed list saves are covered by the existing 5xx alert and that a failure is found in the logs by its trace ID, without the title. Verify by forcing one locally.
- [ ] 2.5 Add the Lists navigation entry, with `DashboardNav` tests, and update README and `backend/ARCHITECTURE.md`. Get `just ci` green. Note for the first deploy: `just smoke <url>`, then open Lists from the navigation.
- [ ] 2.6 Revise design.md with what both slices showed and add the remaining slices here. They must cover: renaming and deleting, with their conflicts; ordering by drag and keyboard; the mobile drawer and touch; full lists of 1,000; the streamed export.

## 3. Feature done

- [ ] 3.1 Every scenario in the spec deltas is delivered by a slice above and has a test; list any exception with its reason. Verify by walking the specs against the tests.
- [ ] 3.2 README, `backend/ARCHITECTURE.md` and the threat model match what was built, and the first-deploy checks are recorded. There is no flag to remove (D11). Verify with `just ci`.
- [ ] 3.3 Archive the change with `/opsx:archive`, and verify with `openspec validate --specs --strict`.
