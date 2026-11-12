# Tasks

No production deploy exists yet. A slice "ships" when it's merged to `main` with
`just ci` green and checked in the full local stack (`just dev` plus
`just obs-up`). Production steps are collected in group 2 for the first deploy.

Slices 2–4 (design "Open Questions → Planned slices") are added after slice 1
has shipped and `design.md` has been revised with what it showed.

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

- [ ] 1.1 Add `application/dto/client_telemetry_dto.rs` (Decision 2), covering only the `error` event variant in this slice, plus the `ClientErrorKind` enum in `schema_enums.rs`. Verify: DTO validation unit tests reject 31 events, a 501-character message, a 4,001-character stack, a malformed `page_load_id` and a malformed `trace_id`, and accept a valid batch.
- [ ] 1.2 Add the incoming port `RecordClientTelemetry`, the outgoing port `ClientReportSink`, and the use case `application/use_cases/telemetry/record_client_telemetry.rs`. The use case covers the page allowlist → `other`, the device-class and browser-family heuristic, server-side scrubbing (strip query and fragment from URL-like substrings, `[EMAIL]`, `redact_query`, truncation), fingerprinting with `shared::fingerprint`, and the token-bucket budget (`CLIENT_TELEMETRY_MAX_EVENTS_PER_SEC`, default 200). Verify: mock-sink unit tests cover each rule, including over-budget events going to `dropped(n)` and never to `error(...)`.
- [ ] 1.3 Add `backend/tests/fixtures/client_scrub_cases.json` with input → expected pairs: `/verify-magic#token=…`, `/reset-password?email=…`, an email in a message, an absolute URL with a query inside a stack frame, and `key=value` secrets. Load it in the use case tests. Verify: `just test` runs every fixture case.
- [ ] 1.4 Add the `ClientReportSink` adapter in `infrastructure/telemetry/client_reports.rs`: a `warn!` with `event = "client_error"` and explicit fields (Decision 2), and a `browser.report.dropped` counter. Verify: a unit test with the in-memory exporter sees the dropped counter, and a log-capture test sees every field, plus `user_id` only when it's given.
- [ ] 1.5 Add `client_telemetry_handler.rs` (`OptionalAuthUser`, `User-Agent`, `ValidatedJson`, 204). Mount it in its own group merged below `require_csrf()` with a 16 KB `DefaultBodyLimit`, and update the comment there. Add the route to an `UNTRACED_ROUTES` list in `telemetry/http.rs`, still recording request duration. Wire everything in `main.rs` and add the setting to `settings.rs`. Then run `just api-types` and commit `backend/openapi.json` and `schema.d.ts`. Verify: `just lint` passes and `just api-types-check` is clean.
- [ ] 1.6 Add HTTP integration tests in `backend/tests/client_telemetry_integration.rs`. Verify with `just test-integration` that they cover:
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

- [ ] 1.7 Add `src/lib/telemetry/scrub.ts`, driven by the same `client_scrub_cases.json` fixture. Verify: `just test-web` runs every fixture case with identical expected output.
- [ ] 1.8 Add `src/lib/telemetry/reporter.ts` and `index.ts` (Decision 8), covering:
  - `error` and `unhandledrejection` handlers;
  - dedupe with a count, and the cap of 20 distinct errors per page load;
  - a page-load ID held in memory only, regenerated on a sign-in/sign-out change in `session.svelte.ts`;
  - the batch splitter at 16 KB;
  - flushing on `visibilitychange: hidden`, on `pagehide`, and every 10 s;
  - `sendBeacon` with a `fetch` keepalive fallback;
  - never using `HttpClient`;
  - every entry point wrapped so the reporter never throws into the page.

  Verify: Vitest unit tests cover dedupe, the cap, ID rotation, splitting, the fallback path, a throwing `sendBeacon` staying silent, and no `localStorage`/`sessionStorage`/`document.cookie` writes.
- [ ] 1.9 Load the reporter from `Base.astro` as a bundled module script, and set `PUBLIC_RELEASE` at build time (`dev` locally; a `RELEASE` build arg in `apps/web/Dockerfile`). Verify: `just test-web`, and the existing `tests/csp.spec.ts` passes with no new hash.
- [ ] 1.10 Add a Playwright flow `tests/browser-errors.spec.ts` (API mocked). Verify with `just e2e` that it covers:
  - throwing on `/login` sends one beacon with page `/login`, release, kind and a scrubbed stack;
  - on `/reset-password?email=a@b.c` and `/verify-magic#token=abc`, a thrown error whose message holds `location.href` leaves neither the email nor `abc` anywhere in the payload;
  - the same error 50× gives one event with count 50;
  - 500 distinct errors give at most 20 events;
  - blocking the endpoint route (abort) leaves sign-in working with no console errors from the reporter;
  - there are no CSP violations;
  - a phone viewport flushes on `visibilitychange`.

### Source maps

- [ ] 1.11 Set `vite.build.sourcemap: 'hidden'`. In `apps/web/Dockerfile`, move `dist/**/*.map` out of the served tree to `/sourcemaps/<RELEASE>/` in the build stage. In CI, upload them as `web-sourcemaps-<sha>` (90-day retention). Verify: `docker run` of the built web image serves 404 for every `.map` path listed in the build (add the check to `tests/caddy-headers.spec.ts` or a new spec run against the built image), and no built JS file contains `sourceMappingURL`.
- [ ] 1.12 Add `scripts/decode-stack` (Node, `source-map` devDependency) and `just decode-stack <release> [file]`. It reads the stack from a file or stdin, uses `apps/web/dist` for `dev`, and otherwise fetches `web-sourcemaps-<release>` with `gh run download` into a local cache. Verify: decoding a stack from a local release build maps the top frame to the `src/…` line that threw (a scripted check in `just test-web` against a fixture build).

### Observability

- [ ] 1.13 Add `deploy/observability/grafana/dashboards/frontend.json` with this slice's panels:
  - browser errors over time by page and release (VictoriaLogs);
  - top fingerprints with first and last seen times;
  - an example-reports table where `client_trace_id` links to Tempo when present;
  - `browser.report.dropped`.

  Verify: `just obs-check` passes, and the dashboard loads in local Grafana with data from 1.14.
- [ ] 1.14 Check the hypothesis end to end in the full local stack. On a release-mode web build behind the edge, throw on `/reset-password?email=…` and `/verify-magic#token=…` signed out, and on `/dashboard/settings` signed in. Confirm in Grafana that all three reports appear within 1 minute, that the dashboard one carries the Member's `user_id`, that a VictoriaLogs search for the email and the token finds nothing, and that `just decode-stack` decodes a logged stack to the right source line. Record the result in `design.md` → Hypothesis.
- [ ] 1.15 Update `CLAUDE.md`: below `require_csrf()` now allows "public, state-free browser reports" as well as signature-verified webhooks. Update `README.md` with browser telemetry, `PUBLIC_RELEASE`, `CLIENT_TELEMETRY_MAX_EVENTS_PER_SEC` and `just decode-stack`. Verify: the docs read correctly, and `just ci` is green.
- [ ] 1.16 Merge slice 1 with `just ci` green, then revise `design.md` (Hypothesis outcome, any changed decisions) and add slices 2–4 to this file.

## 2. Feature done

- [ ] 2.1 Before or at the first production deploy:
  - add a Cloudflare rate-limiting rule for `/api/v1/client-telemetry` per client IP;
  - pass `RELEASE=<sha>` to the production web image build;
  - set `PUBLIC_RUM_SAMPLE_RATE` for production;
  - check with `just smoke <url>` and a deliberate error on a staging page that a report reaches production Grafana and `just decode-stack` decodes it.

  Record these in the deploy README's first-deploy checklist.
- [ ] 2.2 Confirm the observability spec delta was merged cleanly with whatever the business-metrics change archived (both touch `openspec/specs/observability/spec.md`), then archive this change.
