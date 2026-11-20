# Tasks

No production deploy exists yet. A slice "ships" when it's merged to `main` with
`just ci` green and checked in the full local stack (`just dev` plus
`just obs-up`). Production steps are collected in group 2 for the first deploy.

## 1. Slice 1 (first slice): uncaught browser errors end to end, scrubbed, attributed and decodable

This slice delivers these scenarios.

From `specs/observability/spec.md`:
- **Browser errors are reported**: all scenarios except those needing panel
  boundaries (slice 2): uncaught error on a public page, error from a signed-in
  Member, expired session, user ID in the report body is ignored, unknown page,
  the same error repeats, an error loop on one page, events from different page
  loads are grouped, signing in or out starts a new page-load ID.
- **Browser reports never expose secrets or personal data**: all scenarios.
- **Browser reporting never affects the page**: all scenarios.
- **The browser report endpoint is bounded**: malformed report, oversized
  report, flood of reports. "Unknown metric values cannot create series" comes
  in slice 2, with the first metrics.
- **Operator sees and is alerted to browser problems**: decode a minified
  stack, source maps are not public.
- **Every request is traceable** (modified): browser reports do not flood
  traces.
- **Observability tools are restricted to the operator** (modified): the
  browser report endpoint cannot read telemetry.

### Backend

- [x] 1.1 Add `application/dto/client_telemetry_dto.rs` (Decision 2), covering only the `error` event variant in this slice, plus the `ClientErrorKind` enum in `schema_enums.rs`. Verify: DTO validation unit tests reject 31 events, a 501-character message, a 4,001-character stack, a malformed `page_load_id` and a malformed `trace_id`, and accept a valid batch.
- [x] 1.2 Add the incoming port `RecordClientTelemetry`, the outgoing port `ClientReportSink`, and the use case `application/use_cases/telemetry/record_client_telemetry.rs`. The use case covers the page allowlist → `other`, the device-class and browser-family heuristic, server-side scrubbing (strip query and fragment from URL-like substrings, `[EMAIL]`, `redact_query`, truncation), fingerprinting with `shared::fingerprint`, and the token-bucket budget (`CLIENT_TELEMETRY_MAX_EVENTS_PER_SEC`, default 200). Verify: mock-sink unit tests cover each rule, including over-budget events going to `dropped(n)` and never to `error(...)`.
- [x] 1.3 Add `backend/tests/fixtures/client_scrub_cases.json` with input → expected pairs: `/verify-magic#token=…`, `/reset-password?email=…`, an email in a message, an absolute URL with a query inside a stack frame, and `key=value` secrets. Load it in the use case tests. Verify: `just test` runs every fixture case.
- [x] 1.4 Add the `ClientReportSink` adapter in `infrastructure/telemetry/client_reports.rs`: a `warn!` with `event = "client_error"` and explicit fields (Decision 2), and a `browser.report.dropped` counter. Verify: a unit test with the in-memory exporter sees the dropped counter, and a log-capture test sees every field, plus `user_id` only when it's given.
- [x] 1.5 Add `client_telemetry_handler.rs` (`OptionalAuthUser`, `User-Agent`, `ValidatedJson`, 204). Mount it in its own group merged below `require_csrf()` with a 16 KB `DefaultBodyLimit`, and update the comment there. Add the route to an `UNTRACED_ROUTES` list in `telemetry/http.rs`, still recording request duration. Wire everything in `main.rs` and add the setting to `settings.rs`. Then run `just api-types` and commit `backend/openapi.json` and `schema.d.ts`. Verify: `just lint` passes and `just api-types-check` is clean.
- [x] 1.6 Add HTTP integration tests in `backend/tests/client_telemetry_integration.rs`. Verify with `just test-integration` that they cover:
  - a Visitor report is logged with no `user_id`;
  - a report with a valid `auth_token` cookie is logged with that user's ID;
  - an expired or invalid cookie is logged with no `user_id` and never returns 401;
  - a body-supplied `user_id` field is rejected by the schema or ignored;
  - malformed input returns 400 with the standard error body;
  - more than 16 KB returns 413;
  - over-budget input returns 204 and increments the dropped counter;
  - no CSRF header is needed;
  - no span is exported for the route, but the request duration metric is recorded;
  - the response body is empty.

### Web

- [x] 1.7 Add `src/lib/telemetry/scrub.ts`, driven by the same `client_scrub_cases.json` fixture. Verify: `just test-web` runs every fixture case with identical expected output.
- [x] 1.8 Add `src/lib/telemetry/reporter.ts` and `index.ts` (Decision 8), covering:
  - `error` and `unhandledrejection` handlers;
  - dedupe with a count, and the cap of 20 distinct errors per page load;
  - a page-load ID held in memory only, regenerated on a sign-in/sign-out change in `session.svelte.ts`;
  - the batch splitter at 16 KB;
  - flushing on `visibilitychange: hidden`, on `pagehide`, and every 10 s;
  - `sendBeacon` with a `fetch` keepalive fallback;
  - never using `HttpClient`;
  - every entry point wrapped so the reporter never throws into the page.

  Verify: Vitest unit tests cover dedupe, the cap, ID rotation, splitting, the fallback path, a throwing `sendBeacon` staying silent, and no `localStorage`/`sessionStorage`/`document.cookie` writes.
- [x] 1.9 Load the reporter from `Base.astro` as a bundled module script, and set `PUBLIC_RELEASE` at build time (`dev` locally; a `RELEASE` build arg in `apps/web/Dockerfile`). Verify: `just test-web`, and the existing `tests/csp.spec.ts` passes with no new hash.
- [x] 1.10 Add a Playwright flow `tests/browser-errors.spec.ts` (API mocked). Verify with `just e2e` that it covers:
  - throwing on `/login` sends one beacon with page `/login`, release, kind and a scrubbed stack;
  - on `/reset-password?email=a@b.c` and `/verify-magic#token=abc`, a thrown error whose message holds `location.href` leaves neither the email nor `abc` anywhere in the payload;
  - the same error 50× gives one event with count 50;
  - 500 distinct errors give at most 20 events;
  - blocking the endpoint route (abort) leaves sign-in working with no console errors from the reporter;
  - there are no CSP violations;
  - a phone viewport flushes on `visibilitychange`.

### Source maps

- [x] 1.11 Set `vite.build.sourcemap: 'hidden'`. In `apps/web/Dockerfile`, move `dist/**/*.map` out of the served tree to `/sourcemaps/<RELEASE>/` in the build stage. In CI, upload them as `web-sourcemaps-<sha>` (90-day retention). Verify: `docker run` of the built web image serves 404 for every `.map` path listed in the build (add the check to `tests/caddy-headers.spec.ts` or a new spec run against the built image), and no built JS file contains `sourceMappingURL`.
- [x] 1.12 Add `scripts/decode-stack` (Node, `source-map` devDependency) and `just decode-stack <release> [file]`. It reads the stack from a file or stdin, uses `apps/web/dist` for `dev`, and otherwise fetches `web-sourcemaps-<release>` with `gh run download` into a local cache. Verify: decoding a stack from a local release build maps the top frame to the `src/…` line that threw (a scripted check in `just test-web` against a fixture build).

### Observability

- [x] 1.13 Add `deploy/observability/grafana/dashboards/frontend.json` with this slice's panels:
  - browser errors over time by page and release (VictoriaLogs);
  - top fingerprints with first and last seen times;
  - an example-reports table where `client_trace_id` links to Tempo when present;
  - `browser.report.dropped`.

  Verify: `just obs-check` passes, and the dashboard loads in local Grafana with data from 1.14.
- [x] 1.14 Check the hypothesis end to end in the full local stack. On a release-mode web build behind the edge, throw on `/reset-password?email=…` and `/verify-magic#token=…` signed out, and on `/dashboard/settings` signed in. Confirm in Grafana that all three reports appear within 1 minute, that the dashboard one carries the Member's `user_id`, that a VictoriaLogs search for the email and the token finds nothing, and that `just decode-stack` decodes a logged stack to the right source line. Record the result in `design.md` → Hypothesis.
- [x] 1.15 Update `CLAUDE.md`: below `require_csrf()` now allows "public, state-free browser reports" as well as signature-verified webhooks. Update `README.md` with browser telemetry, `PUBLIC_RELEASE`, `CLIENT_TELEMETRY_MAX_EVENTS_PER_SEC` and `just decode-stack`. Verify: the docs read correctly, and `just ci` is green.
- [x] 1.16 Merge slice 1 with `just ci` green, then revise `design.md` (Hypothesis outcome, any changed decisions) and add slices 2–4 to this file.

## 2. Slice 2: panel boundaries, API failures, error rate and its alert

This slice delivers these scenarios.

From `specs/dashboard/spec.md`:
- **A crashing panel is contained**: all scenarios.

From `specs/observability/spec.md`:
- **Failed API calls in the browser link to the backend trace**: all scenarios.
- **Real-user performance is measured**: the page-load counting in "Navigating
  inside the dashboard" (Web Vitals come in slice 3).
- **The browser report endpoint is bounded**: "Unknown metric values cannot
  create series" for page and panel names.
- **Operator sees and is alerted to browser problems**: error spike after a
  release, no alert on low traffic, and the error-rate part of the dashboard.

### Backend

- [x] 2.1 Add the `page_load` event variant and the optional `island` field on error events (Decision 2), with the island allowlist read from a shared fixture `backend/tests/fixtures/client_islands.json`. Map unknown islands to `other`. Verify: DTO and use case unit tests cover a valid `page_load`, an unknown island → `other`, and an over-long island name rejected.
- [x] 2.2 Add `page_load(...)` to `ClientReportSink`, and the counters `browser.page_load` (`page`, `device_class`, `authenticated` from the session) and `browser.error` (`page`, `kind`, `release`, incremented by `count`, same-origin only, Decision 7). The use case caps the `release` label at 5 distinct values per replica, its own `APP_VERSION` included, and maps the rest to `other` in `client_reports.rs`. Log `island` on `client_error` when present. Verify: in-memory exporter tests see both counters with only enum or allowlist label values, a sixth release labelled `other` while its log line keeps the real value, an external error logged but not counted, and `authenticated` ignoring anything in the body.
- [x] 2.3 Extend `client_telemetry_integration.rs`. Verify with `just test-integration`: a signed-in `page_load` counts `authenticated=true`; a `boundary` error is logged with its island; an `api` error is logged with `api_route`, `status` and `client_trace_id`; made-up pages and islands are recorded as `other`. Then `just api-types` and commit `openapi.json` and `schema.d.ts`.

### Web

- [x] 2.4 Add `lib/telemetry/TelemetryBoundary.svelte` (Decision 8, without timing): a `<svelte:boundary>` with a required `name`, a fallback with "Try again" (calls `reset`), no error details, and a `kind: boundary` report with the island name. Verify: Vitest browser tests cover a crash showing the fallback, Try again rendering the panel, Try again failing again showing the fallback once more with a second report, no message or stack in the DOM, and a phone-width fallback.
- [x] 2.5 Wrap the root of every island (`AuthScreen`, `ForgotPasswordForm`, `ResetPasswordForm`, `SetPasswordForm`, `VerifyEmailForm`, `VerifyMagicForm`, `HomePanel`, `SettingsPanel`, `AdminPanel`, `DashboardHeader`, `ModeWatcher`) in `TelemetryBoundary`. Add the Decision 6b guard test: every `client:*` component in `src/pages` and `src/layouts` renders a boundary, and the set of names equals `client_islands.json`. Verify: `just test-web`, and the guard fails when a boundary is removed.
- [x] 2.6 Emit a `page_load` event on every `astro:page-load` with the current page, never sampled. Verify: Vitest sees one event per full load and per swap, each with the page it landed on.
- [x] 2.7 Add `reportApiFailure({ route, status | network_failed, traceId })` to `lib/telemetry/index.ts`. Have `lib/api/client.ts` call it for 5xx and network failures only, with the path template passed by the `lib/api/*` wrappers. Verify: `client.test.ts` covers a 500 reporting route, status and `X-Trace-Id`; a network error reporting `network_failed` and no trace ID; 401 and 422 not reporting; and a throwing reporter leaving the call's result unchanged.
- [x] 2.8 Add Playwright flows (API mocked). Islands are crashed through `TelemetryBoundary`'s test hook (`window.__otCrashIsland`), built in only by `astro dev` and `PUBLIC_TELEMETRY_TEST_HOOKS=true` (`just e2e`, CI's e2e job), plus one real crash from bad data. Verify with `just e2e` that they cover:
  - the Settings panel throwing: the fallback is shown and the header and navigation still work; Try again succeeds, then fails again;
  - the header throwing while the panel keeps working;
  - the sign-in form throwing on `/login`;
  - a phone viewport fallback;
  - changing the password with a mocked 500 sending one `api` event with the route, 500 and the trace ID (Settings has no name form);
  - Register with the API aborted sending the network failure once the route is reachable again or the page is hidden;
  - a 401 login sending no error event;
  - no CSP violations.

### Observability

- [x] 2.9 Add to `frontend.json` the metric-based error rate (`browser.error` / `browser.page_load`) per page and release, with the dashboard description saying page loads are an error-rate denominator, not traffic analytics (Decision 12). Add the `frontend` alert group with `BrowserErrorRateHigh` (Decision 11) in `alerts/overkill-todo.yml`, with tests in `alerts/tests/overkill-todo.test.yml` for the 5% spike firing, the resolved notice, and 1 error in 3 page loads not firing. Verify: `just obs-check`.
- [x] 2.10 Check in the full local stack: stop Postgres, change the password on `/dashboard/settings`, and confirm the `api` report appears in Grafana with the route, 500 and a `client_trace_id` that opens the backend trace in Tempo in one click. Under `just dev`, set `window.__otCrashIsland = 'SettingsPanel'` in the console, choose Try again, and confirm the boundary report names the island. Record the result in `design.md`.
- [x] 2.11 Update `backend/ARCHITECTURE.md` or the web README with "every island root renders `TelemetryBoundary`". Merge with `just ci` green.

## 3. Slice 3: Web Vitals, sampling and the poor-vitals alert

This slice delivers these scenarios.

From `specs/observability/spec.md`:
- **Real-user performance is measured**: Web Vitals per page, compare releases,
  sampling, navigating inside the dashboard (the per-visit part), leaving the
  page on a phone.
- **The browser report endpoint is bounded**: unknown measurement names and
  ratings are rejected.
- **Operator sees and is alerted to browser problems**: slow page alert, and the
  Web Vitals part of the dashboard.

- [x] 3.1 Add the `vital` event variant with the `VitalName` and `Rating` enums, plus a poor-rating attribution field (tag name and selector, 100 characters, Decision 4). Add `vital(...)` to the sink and the `browser.web_vital` histogram (Decision 7) with explicit buckets around each threshold. Log attribution only for poor ratings. Verify: unit tests reject an unknown name and rating, place values in the expected buckets, and log attribution only when poor; integration tests record a vital per page; `just api-types` committed.
- [x] 3.2 Add `PUBLIC_RUM_SAMPLE_RATE` to `public/config.js`, `docker-entrypoint.d/40-render-public-config.sh` and `lib/config.ts`, defaulting to `1` when unset and treating invalid values as `1`. Verify: `config.test.ts` covers unset, `0`, `0.1` and junk.
- [x] 3.3 Add `lib/telemetry/vitals.ts`: `web-vitals` (attribution build) for LCP, FCP and TTFB on the full load, and our own INP and CLS observers per visit (Decision 6), sampled once per full load. Verify: Vitest feeds recorded entry sequences and, for a full load with no swaps, matches `web-vitals`' INP and CLS; a visit ends at `astro:before-preparation` and on hide; a rate of `0` sends no vitals but still sends errors and page loads.
- [x] 3.4 Add Playwright flows. Verify with `just e2e`: Home → Settings with an interaction reports INP and CLS for `/dashboard/settings` and LCP, FCP and TTFB only for `/dashboard`; a phone viewport flushes vitals on `visibilitychange`; a sample rate of `0` sends no vitals; no CSP violations.
- [x] 3.5 Add `frontend.json` panels for p75 per vital per page and device class, and rating shares. Add `BrowserVitalPoor` (Decision 11) with tests for INP over 500 ms with 200 samples firing, and 30 samples not firing. Verify: `just obs-check`.
- [x] 3.6 Add the `browser.release_seen` counter (`release`), incremented in the sink's `page_load` (Decision 7). In `frontend.json`, add a release annotation from each release's first sample, and a panel comparing p75 per vital for the 24 h before and after the selected release, next to the error rate per release. Verify: a unit test sees one series per release; `just obs-check`; and in local Grafana, two builds with different `RELEASE` loaded one after the other show two annotations, with the vitals split at the second one.
- [x] 3.7 Check in the full local stack on a release build: load public pages and the dashboard on desktop and phone emulation, and confirm p75 per page and device class within a few minutes. Record the result in `design.md`. Merge with `just ci` green.

## 4. Slice 4: in-site navigation and panel timing

This slice delivers these scenarios.

From `specs/observability/spec.md`:
- **In-site navigations are measured**: all scenarios.
- **Panel loading is measured**: all scenarios.
- **The browser report endpoint is bounded**: unknown navigation kinds,
  transition results and phases are rejected; made-up panel names become
  `other`.
- **Operator sees and is alerted to browser problems**: slow navigation alert,
  and the navigation and panel parts of the dashboard.

- [ ] 4.1 Add the `navigation` and `island` event variants with the `NavType`, `ViewTransitionResult`, `NavOutcome` and `IslandStage` enums, and the instruments `browser.navigation.duration`, `browser.navigation.phase.duration`, `browser.navigation` and `browser.island.duration` (Decision 7). Verify: unit tests reject unknown enum values, map unknown islands to `other`, and assert every label is from an enum or allowlist; integration tests record each variant; `just api-types` committed.
- [ ] 4.2 Add `lib/telemetry/navigation.ts` (Decision 6a): marks t0–t4, the phases, the `view_transition` label, aborted navigations, the full-reload fallback dropped on `pagehide`, and full loads as `hard`. Verify: Vitest drives the lifecycle events in order, interrupted, and with no `startViewTransition`.
- [ ] 4.3 Add timing to `TelemetryBoundary` (Decision 6b): **interactive** in `onMount` relative to the current navigation, `markReady()` through context (once only), and both marks dropped when a newer navigation started. Verify: Vitest covers a full load, a swap, a second `markReady()` ignored, and a superseded navigation dropping its marks.
- [ ] 4.4 Call `markReady()` in every panel when it first shows content, an empty state or an error state, and in `onMount` for panels with no data. Verify: each panel's Vitest test asserts exactly one call for each of its states.
- [ ] 4.5 Add a Chromium Playwright flow. Verify with `just e2e`: Home → Settings, Settings → Admin interrupted, and back report ordered phases with the right labels; with `startViewTransition` removed it reports `unsupported`; a forced full-reload fallback reports a `hard` navigation; the persisted header is timed only on the full load; a panel whose data call fails reports **ready** at its error state; leaving before ready sends no ready mark; a phone viewport reports `device_class=mobile`.
- [ ] 4.6 Add `frontend.json` panels for navigations (p75 total per destination and kind, stacked phases, transition outcome shares, aborted share) and panels (p75 interactive and ready per panel and kind). Add `BrowserNavigationSlow` (Decision 11) with tests for `/dashboard/admin` over 1 s with 120 navigations firing, and 30 navigations not firing. Verify: `just obs-check`.
- [ ] 4.7 Check in the full local stack: navigate around the dashboard and confirm navigations and panel timings appear per page and panel. Record the result in `design.md`. Merge with `just ci` green.

## 5. Feature done

- [ ] 5.1 Before or at the first production deploy:
  - add a Cloudflare rate-limiting rule for `/api/v1/client-telemetry` per client IP;
  - pass `RELEASE=<sha>` to the production web image build;
  - set `PUBLIC_RUM_SAMPLE_RATE` for production;
  - check with `just smoke <url>` and a deliberate error on a staging page that a report reaches production Grafana and `just decode-stack` decodes it.

  Record these in the deploy README's first-deploy checklist.
- [ ] 5.2 Update `README.md` and `backend/ARCHITECTURE.md` for anything slices 2–4 changed (`PUBLIC_RUM_SAMPLE_RATE`, the boundary rule for new islands, the alerts). Verify: the docs read correctly.
- [ ] 5.3 Confirm the observability spec delta was merged cleanly with whatever the business-metrics change archived (both touch `openspec/specs/observability/spec.md`), then archive this change.
