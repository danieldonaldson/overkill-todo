# Design

## Context

See `proposal.md` (Why) and the two spec deltas for the required behaviour.
This design builds on OT-001's telemetry stack (archived design
`2026-11-12-ot-001-observability`): the backend writes JSON logs to stdout, and
the collector ships them to VictoriaLogs. Metrics are pushed over OTLP to
VictoriaMetrics, alerts are vmalert rules tested by `just obs-check`, and
Grafana is reached only through the operator's tunnel.

Constraints that shape the design:

- **Strict CSP** (`apps/web/Caddyfile`): `script-src 'self'` plus two pinned
  hashes, and `connect-src 'self'`. Same-origin external scripts and
  same-origin requests need no CSP change.
- **Navigation:** the site uses Astro's `ClientRouter`, so dashboard navigation
  swaps pages without a reload. Module state survives these navigations, and
  the dashboard header is `transition:persist`.
- **Auth cookie:** the session is the `HttpOnly` `auth_token` cookie with
  `Path=/` and `SameSite=Lax`. `OptionalAuthUser`
  (`infrastructure/api/middleware/auth.rs`) resolves it without ever rejecting.
- **Routing:** the edge routes `/api/*` to the backend. Public routes are merged
  below `require_csrf()` in `api_routes()`.
- **Edge image:** stock `caddy:2`, which has no rate-limit module.
- **Existing helpers:** `shared/fingerprint.rs` already normalises and hashes
  error messages for grouping. `HttpClient` already captures `X-Trace-Id`.
- **OT-005 (web analytics)** is being planned in parallel. It adds its own
  loader and a `PUBLIC_*` setting, and promises no browser storage and no
  identifiers for analytics.

## Goals / Non-Goals

**Goals:**
- A browser error and Web Vitals pipeline that reuses the OT-001 storage,
  dashboards and alerts, with no new running service.
- Every privacy rule enforced twice: in the browser before sending, and on the
  server before recording. The collector's redaction is a third net.
- The backend endpoint is self-contained, so it can later move into its own
  service without touching other features.

**Non-Goals:**
- LCP, FCP and TTFB per in-site navigation. They need the experimental
  soft-navigation API, so they stay per full page load. INP and CLS *are*
  measured per page visit, and navigations and panels have their own timings
  (Decisions 6, 6a, 6b).
- Server-side symbolication of stacks.
- Per-IP rate limiting in this change (Decision 9).

## Hypothesis

We believe a browser error report can be made both actionable and safe with
no third-party tool:

- the build produces source maps that are kept out of the served image but
  saved per release, so a logged minified stack decodes to source lines;
- scrubbing in the browser, then again on the server, removes every
  query string and fragment wherever a URL appears.

**Slice 1 tests this end to end**, in the full local stack and on a release
image:
- a Playwright flow opens `/reset-password?email=…` and `/verify-magic#token=…`
  (the real link formats: the email in the query string, the magic token in
  the fragment) and throws an error whose message and stack contain the full
  address;
- the test then searches VictoriaLogs for the email address and the token;
- `just decode-stack` turns the logged stack into `src/…` file and line
  numbers.

**We will know it works when:**
- neither the email address nor the token is ever found;
- the decoded frames point at the line that threw;
- no `.map` file is reachable from the web image.

If decoding through the Docker build turns out awkward, fall back to uploading
maps from CI only and record that here before slice 2.

## Decisions

### 1. In-house reporter plus a backend endpoint

Browser → `POST /api/v1/client-telemetry` → backend → existing stdout-log and
OTLP-metric paths.

**Why:**
- No telemetry ingestion endpoint becomes public. The spec keeps that rule and
  adds a clarifying sentence.
- Typed validation stops input from creating unlimited metric series.
- `OptionalAuthUser` attributes the user for free.
- Redaction, limits and tests reuse what already exists.

**Alternatives considered:**
- **Edge → a dedicated ingest-only collector:** no user attribution, weaker
  validation, and a public ingestion path.
- **Grafana Faro SDK + an Alloy `faro.receiver`:** a second agent next to the
  collector and a heavier bundle.
- **Sentry / GlitchTip:** a third-party processor, or a whole extra stack to
  run.
- **OTel JS browser SDK:** tens of KB, and its main payoff (browser-started
  traces) is excluded by the spec.

**Accepted cost:** the backend being down hides the errors it causes. OT-001's
`BackendDown` alert and the edge access logs already cover that case.

### 2. Backend layering

The handler is thin and the use case is mockable, following the established
pattern:

- **`application/dto/client_telemetry_dto.rs`**
  - `ClientReportBatch { page_load_id, release, page, authenticated_hint?, events: Vec<ClientEvent> }`.
  - `ClientEvent` is a tagged enum: `error { kind, message, stack?, source?, api_route?, status?, network_failed?, trace_id?, count }`, `vital { name, value, rating }`, `page_load`, `navigation { type, view_transition, outcome, total_ms?, fetch_ms?, swap_ms?, transition_ms? }` and `island { name, stage, nav_type, ms }`. Each variant is added in the slice that uses it.
  - Validation uses `#[validate]` limits:
    - at most 30 events per batch;
    - message at most 500 characters, stack at most 4,000;
    - `page_load_id` is 32 hex characters;
    - `trace_id` is 32 lowercase hex characters.
  - Fixed value sets use `schema_enums.rs` enums: `VitalName` (LCP, INP, CLS, FCP, TTFB), `Rating`, error `kind` (`error`, `unhandledrejection`, `boundary`, `api`), `NavType`, `ViewTransitionResult`, `NavOutcome` and `IslandStage`.
  - Island names, like pages, are free strings mapped to an allowlist on the server, falling back to `other`.
  - `page` is a free string, mapped to an allowlist on the server (Decision 5).
- **`application/ports/incoming/record_client_telemetry.rs`**: one method,
  `record(batch, user: Option<UserId>, user_agent: &str)`.
- **`application/ports/outgoing/client_report_sink.rs`**: `ClientReportSink`,
  with methods `error(...)`, `vital(...)`, `page_load(...)` and `dropped(n)`,
  taking already-normalised values.
- **`application/use_cases/telemetry/record_client_telemetry.rs`**: the rules.
  1. Map `page` to the allowlist or `other`.
  2. Derive `device_class` and `browser_family` from the user agent with a
     small heuristic (no UA-parsing crate).
  3. Re-scrub every text field (Decision 4).
  4. Fingerprint errors with `shared::fingerprint::generate_error_fingerprint("client:<kind>", message + top frame)`.
  5. Apply the recording budget (Decision 9).
  6. Call the sink.

  It is pure logic, so its unit tests use a mock sink.
- **`infrastructure/telemetry/client_reports.rs`**: the `ClientReportSink`
  adapter.
  - **Errors** become `tracing::warn!` with `event = "client_error"` and
    explicit fields: page, release, kind, fingerprint, count, message, stack,
    api_route, status, network_failed, `client_trace_id`, page_load_id,
    browser_family, device_class and `user_id` when present.
  - **Vitals and page loads** go to OTel instruments (Decision 7).
  - **Dropped reports** go to a counter.
- **`infrastructure/api/handlers/client_telemetry_handler.rs`**:
  - extracts `OptionalAuthUser`, the `User-Agent` header and
    `ValidatedJson<ClientReportBatch>`;
  - calls the use case;
  - always returns `204 No Content` on success, including when reports are
    dropped.

**Why a port here when OT-001 Decision 2 avoided one:** there, nothing in the
application layer emitted telemetry. Here the use case owns real rules
(allowlist, scrub, budget) that deserve mock-based tests. The sink has no
no-op variant to refuse in production: it always logs, and its metric
instruments are no-ops exactly when OT-001's exporter is (already refused in
production).

**`authenticated_hint` is not trusted for identity.** The `authenticated`
metric label comes from `OptionalAuthUser` on the server. The hint field is
dropped from the DTO before implementation if unused.

### 3. Route placement, CSRF, size and tracing

- **Placement:** mounted in its own group, merged below `require_csrf()` next to
  `health`:

  ```rust
  .require_csrf()
  .merge(health)
  .merge(client_telemetry)
  ```

  `sendBeacon` cannot set the CSRF header, and the endpoint changes no state
  and returns nothing. CSRF protects state-changing actions taken with a
  victim's cookies, and the worst a forged report can do is add noise under
  that user's ID, which the budget bounds. The comment in `api_routes()` and
  the CLAUDE.md rule ("public signature-verified webhooks go below it") are
  extended to name "public, state-free browser reports" as the second category.
- **Body limit:** a `DefaultBodyLimit::max(16 * 1024)` route layer gives 413.
- **No trace:** the route is added to the span skip list in
  `telemetry/http.rs` (with `HEALTH_ROUTES`, as a separate `UNTRACED_ROUTES`
  list). It still records `http.server.request.duration`, per the modified spec
  scenario. Error logs carry the *client's* trace ID as `client_trace_id`, and
  the Grafana derived field links that to Tempo.
- **Errors:** the handler logs nothing at error level for bad input. 400s
  appear in the request metrics only, so junk can't flood logs.

### 4. Scrubbing, twice

**In the browser** (`lib/telemetry/scrub.ts`), applied to message, stack,
`source` file name and every URL-like substring:

1. Replace every `https?://…` or `/…` path with its origin + path (drop `?…`
   and `#…`).
2. Replace emails with `[EMAIL]`.
3. Truncate (500 / 4,000).

The page is sent as `location.pathname` only.

**On the server**, the use case repeats steps 1–3 with the same regexes, plus
OT-001's `redact_query` for any `key=value` pairs that survive. The server copy
is authoritative; the client copy keeps secrets from ever leaving the browser.
Shared fixtures (`backend/tests/fixtures/client_scrub_cases.json`, read by both
the Rust and Vitest tests) keep the two in sync.

**Form values are never collected:** the reporter reads only error objects,
API route templates and Web Vitals. It doesn't capture breadcrumbs, the
console or the DOM. Web Vitals attribution is limited to the element's tag
name and the selector `web-vitals` gives, truncated to 100 characters, and
logged only for poor ratings. It isn't a metric label.

### 5. Page templates

Pages are static Astro routes, so the set is small and known.

- **Client:** sends `location.pathname` with any trailing slash removed.
- **Server:** maps it against an allowlist in the use case (`/login`,
  `/register`, `/forgot-password`, `/reset-password`, `/verify-email`,
  `/verify-magic`, `/set-password`, `/dashboard`, `/dashboard/settings`,
  `/dashboard/admin`). Anything else becomes `other`.

A test asserts that the allowlist equals the set of `src/pages/**/*.astro`
routes, so adding a page without updating it fails CI.

### 6. Web Vitals: load metrics per full load, INP and CLS per page visit

**LCP, FCP and TTFB** come from the `web-vitals` library (about 2 KB brotli,
attribution build), for the full page load only. Per-swap versions need the
experimental soft-navigation API.

**INP and CLS** come from our own two `PerformanceObserver`s, because
`web-vitals` accumulates both over the whole document's lifetime, which is
wrong under ClientRouter:
- **INP:** `event` entries (`durationThreshold: 40`), grouped by
  `interactionId`, with the longest duration per interaction. A visit's INP is
  the worst interaction, or the 98th percentile once a visit has more than 50
  interactions, matching the standard definition.
- **CLS:** `layout-shift` entries without `hadRecentInput`, grouped into
  session windows (gap under 1 s, window at most 5 s). A visit's CLS is the
  largest window.

A visit starts at the full load or at `astro:page-load` after a swap. It ends
at `astro:before-preparation` (the next navigation starts) or when the page is
hidden. At the end, the two values are queued with the visit's page and the
counters reset. They're rated with the standard thresholds (INP 200/500 ms, CLS
0.1/0.25), as `web-vitals` does.

**Page loads:** the `page_load` event is emitted on every `astro:page-load`
with the current page, because error rate per page needs the denominator per
page.

**Sampling:** decided once per full load with
`Math.random() < PUBLIC_RUM_SAMPLE_RATE`, and it applies to Web Vitals,
navigations and panels alike. The default is `1` when unset; `0` turns all
three off. Errors and page loads are never sampled.

### 6a. Navigation timing from the ClientRouter lifecycle

Checked in Astro 7.4.7 during planning:
- all five lifecycle events exist;
- `astro:before-swap` carries the `ViewTransition`;
- starting a new navigation calls `skipTransition()` on the running one, which
  rejects its `.ready` with an `AbortError`.

`lib/telemetry/navigation.ts` records `performance.now()` at each event:

| Mark | Event |
|---|---|
| t0 | `astro:before-preparation` (navigation starts; `navigationType` tells link from back/forward) |
| t1 | `astro:after-preparation` (new page fetched) |
| t2 | `astro:before-swap` (attach to `event.viewTransition.ready` and `.finished`) |
| t3 | `astro:after-swap` (DOM swapped) |
| t4 | `astro:page-load` (scripts ran, page usable) |

**Phases:**
- `fetch` = t1 − t0;
- `swap` = t3 − t2 (the DOM update);
- `transition` = from t2 until `finished` settles (the animation; only when it
  ran);
- total = from t0 until the later of t4 and `finished`.

**`view_transition` label:**
- `ran`: `finished` resolved and `ready` didn't reject;
- `skipped`: `ready` rejected, for example superseded or reduced motion;
- `unsupported`: no `document.startViewTransition`, so Astro's fallback swap
  ran.

**Edge cases:**
- **Aborted:** a new t0 arrives before the current navigation's t4. It's
  counted as `outcome=aborted` for the destination it was heading to, with no
  durations.
- **Full-reload fallback:** when Astro gives up on the swap and loads the page
  in full, the new document measures it as a `hard` navigation. The old
  document's open navigation is dropped on `pagehide`.
- **Full loads** go in the same histogram with `type=hard`, timed from
  `timeOrigin` (0) to the first `astro:page-load`. So "time to usable page" is
  comparable across both kinds.

**Why lifecycle events:** they're Astro's public, documented API. The
alternatives, wrapping `navigate()` or patching `startViewTransition`, would
miss back/forward navigations and break on upgrades.

### 6b. Panel timing through a named boundary

The slice-2 boundary wrapper becomes
`lib/telemetry/TelemetryBoundary.svelte`, with a required `name` prop, e.g.
`<TelemetryBoundary name="SettingsPanel">`. Every island's root component
wraps its content in it. It:

- marks **interactive** in `onMount`, relative to the current navigation's
  start: 0 for a full load, t0 for a swap (read from `navigation.ts`);
- provides `markReady()` through Svelte context. A panel calls it once, when it
  first shows content, its empty state or its error state. Later calls are
  ignored;
- drops both marks if a newer navigation has started meanwhile, so a superseded
  navigation's panel isn't recorded;
- contains errors and shows the fallback with "Try again" (dashboard spec);
  this was its original job.

**Persisted header:** `transition:persist` keeps the dashboard header mounted
across swaps, so `onMount` runs only on the full load. That gives the spec's
"measured only when it first appears" with no special case.

**Why not `astro:hydrate` plus the island's `opts` attribute:** both are
undocumented internals, and component URLs are hashed in production builds. An
explicit name gives a stable, allowlisted label and also covers "ready", which
hydration alone can't see.

**Guarding the allowlist:** a Vitest test does two checks:
- it scans `src/pages` and `src/layouts` for `client:*` usages and asserts
  each component's root renders `TelemetryBoundary`;
- it asserts the set of `name`s equals the server's island allowlist (shared
  fixture, like Decision 5's page list).

Panels with no data to load call `markReady()` in `onMount`.

### 7. Metrics

Names follow OT-001 Decision 5 (OTLP names, Prometheus-translated by
VictoriaMetrics):

| Instrument | Type | Labels |
|---|---|---|
| `browser.web_vital` | histogram, unit per vital (`ms`, or `1` for CLS), explicit buckets around the good/poor thresholds | `name`, `page`, `rating`, `device_class`, `authenticated` |
| `browser.page_load` | counter | `page`, `device_class`, `authenticated` |
| `browser.error` | counter (incremented by `count`) | `page`, `kind`, `release` |
| `browser.report.dropped` | counter | none |
| `browser.navigation.duration` | histogram, ms | `page` (destination), `type` (`hard`, `soft`), `view_transition` (`ran`, `skipped`, `unsupported`, `none` for hard), `device_class` |
| `browser.navigation.phase.duration` | histogram, ms | `phase` (`fetch`, `swap`, `transition`), `page`, `device_class` |
| `browser.navigation` | counter | `outcome` (`completed`, `aborted`), `page` |
| `browser.island.duration` | histogram, ms | `island`, `page`, `stage` (`interactive`, `ready`), `nav_type` (`hard`, `soft`), `device_class` |

`release` appears only on `browser.error`. Release-comparison panels for vitals
query logs or use a recording rule over a short window. This keeps vitals
series bounded however many releases accumulate in 30 days. Fingerprints stay
out of metrics entirely; the "top fingerprints" panel is a VictoriaLogs query.

**Series bound:**
- Web Vitals: 5 names × 11 pages × 3 ratings × 3 device classes × 2 = 990
  histograms.
- Navigation duration: 11 pages × 4 type/transition combinations × 3 device
  classes = 132.
- Navigation phases: 3 × 11 × 3 = 99.
- Islands: about 12 islands, each living on one page, × 2 stages × 2
  navigation kinds × 3 device classes = 144.
- Navigation counter: 22.

About 1,400 histograms in total, which is acceptable. The use case's tests
assert every label comes only from the enums and allowlists (pages, islands).

### 8. Browser module layout and transport

`apps/web/src/lib/telemetry/`:
- **`reporter.ts`**: queue, dedupe by fingerprint within the page load (count
  repeats), cap of 20 distinct errors per page load, page-load ID.
- **`scrub.ts`**: Decision 4.
- **`vitals.ts`**: `web-vitals` wiring for the full-load metrics, plus the
  per-visit INP and CLS observers (Decision 6), sampled.
- **`navigation.ts`**: lifecycle-event timing (Decision 6a). It owns "the
  current navigation" (start time and kind) that the other modules read.
- **`TelemetryBoundary.svelte`**: a `<svelte:boundary>` wrapper with the
  fallback UI and "Try again" (calls the boundary's `reset`). It reports
  `kind: boundary`, and handles the panel timing marks and `markReady()`
  context (Decision 6b).
- **`index.ts`**: `installTelemetry()`, idempotent.
  - Registers `error` and `unhandledrejection` handlers.
  - Flushes on `visibilitychange: hidden`, on `pagehide`, and every 10 s while
    there's something queued.
  - Exposes `reportApiFailure()` for `HttpClient`.

**Rules:**
- **Loading:** from `Base.astro` as a bundled `<script>` (Astro emits it as an
  external same-origin module, so no CSP hash). It runs after parse and never
  blocks paint. Errors thrown before it loads are not captured; this is
  accepted, since islands hydrate after it.
- **Transport:** `navigator.sendBeacon('/api/v1/client-telemetry', new Blob([json], { type: 'application/json' }))`.
  If `sendBeacon` is missing or returns false, fall back to
  `fetch(..., { keepalive: true, credentials: 'same-origin' })`.
- **Never `HttpClient`:** reporting doesn't use `HttpClient`, so it can't
  trigger a refresh, a 401 redirect, or report its own failures.
  Batches over 16 KB are split, with oversized stacks trimmed first.
- **Self-protection:** every reporter entry point is wrapped in `try/catch`
  that swallows errors. The reporter never reports its own errors.
- **Page-load ID:** 16 random bytes in hex, held in module memory only, never
  in storage. It's regenerated when the session store in
  `lib/auth/session.svelte.ts` changes between signed in and signed out (a
  small subscription, no change to the session API).
- **Release:** `import.meta.env.PUBLIC_RELEASE`, set at build time from the
  commit SHA (`--build-arg RELEASE`), `dev` otherwise. The same promoted image
  serves every environment, so a build-time value is correct.
- **API failures:** `HttpClient` calls `reportApiFailure({ route, status | network_failed, traceId })`
  only for 5xx or a network error.
  - `route` comes from the call site's path template. The `lib/api/*` wrappers
    pass their template string, e.g. `/admin/users/{id}/role`. Raw paths are
    never sent.
  - 4xx are never reported (spec).

**DNT / GPC are not honoured, unlike OT-005.** These reports are first-party
operational telemetry for keeping the site working: no cross-site tracking,
no browser storage, nothing used for product decisions. Members are already
identified by user ID in backend logs for the same purpose. If the operator
wants GPC to suppress `user_id` attribution, that's one condition in the
reporter; it's recorded here as a deliberate choice, not an oversight.

### 9. Limiting abuse without client IPs

The backend never sees or stores client IPs (OT-001 spec), and stock Caddy
can't rate-limit. So:

- **Client caps:** 20 distinct errors per page load, at most 30 events per
  batch, and flushes no more often than every 10 s.
- **Global server budget:** a token bucket in the use case, at 200 recorded
  events per second per replica, configurable via `CLIENT_TELEMETRY_MAX_EVENTS_PER_SEC`.
  Over budget, events are counted in `browser.report.dropped` and the response
  is still 204.
- **Fixed lists and size caps** stop label explosion and large payloads.
- **At first deploy:** add a Cloudflare rate-limiting rule on
  `/api/v1/client-telemetry` per IP. It's noted in tasks under the
  feature-done group.

A flood can then only crowd out real reports. It can't grow storage without
limit, add series, or slow other routes. Real-IP limits wait for the edge
answer that OT-005's riskiest unknown is working out.

### 10. Source maps

- **Build:** `vite.build.sourcemap: 'hidden'` in `astro.config.mjs`, which
  emits `.map` files with no `sourceMappingURL` comment.
- **Web image:** the Dockerfile's build stage moves `dist/**/*.map` to
  `/sourcemaps/<RELEASE>/` and does not copy them into the Caddy stage.
- **Persistence:** CI uploads them as an artifact
  `web-sourcemaps-<sha>` with 90 days' retention, longer than telemetry's 30.
- **Local builds:** `dist/` keeps them, and `just decode-stack` falls back to
  `apps/web/dist`.
- **Decoding:** `just decode-stack <release> [file]` (script in `scripts/`)
  takes a pasted stack on stdin or from a file. It fetches the artifact with
  `gh run download` if it's not in the local cache, and maps frames with
  `source-map` (a devDependency).
- **Not served:** a Caddy check in `tests/caddy-headers.spec.ts` (or a new
  spec) asserts `*.map` returns 404 from a built image.

### 11. Dashboard and alerts

**Grafana dashboard** `deploy/observability/grafana/dashboards/frontend.json`:
- browser error rate per page and release;
- top fingerprints (logs query), with first and last seen times;
- an example-reports table with a `client_trace_id` link to Tempo;
- p75 per vital per page and device class;
- rating shares;
- navigations: p75 total time per destination page and kind (hard or soft),
  stacked p75 phases (fetch, swap, transition), the share of transitions that
  ran, were skipped or were unsupported, and the share aborted;
- panels: p75 time to interactive and to ready per panel and navigation kind;
- dropped reports.

**Alert rules:** a new group `frontend` in
`deploy/observability/alerts/overkill-todo.yml`:
- **`BrowserErrorRateHigh`:** `sum(rate(browser_error_total[10m])) / sum(rate(browser_page_load_total[10m])) > 0.02`
  and `sum(rate(browser_page_load_total[10m])) * 60 >= 1`, `for: 10m`.
- **`BrowserVitalPoor`:** p75 INP over 500 ms or LCP over 4 s per page, over
  1 h, with at least 50 samples in the hour, `for: 1h`.
- **`BrowserNavigationSlow`:** p75 of
  `browser_navigation_duration_milliseconds{type="soft", page=~"/dashboard.*"}`
  over 1 s per page, over 1 h, with at least 50 completed navigations in the
  hour, `for: 1h`.

**Tests** go in `alerts/tests/overkill-todo.test.yml`, including the
low-traffic case. The group is separate from the backend and business groups,
so the parallel business-metrics change touches different lines.

### 12. Coordination with OT-005 and the business-metrics change

- **`Base.astro`:** OT-005 adds an analytics loader there and this change adds
  the telemetry script. They're independent and both same-origin; whichever
  merges second rebases.
- **`config.js`, `config.ts` and the entrypoint renderer:** each change adds
  one `PUBLIC_*` key. This change uses a *build-time* `PUBLIC_RELEASE` and a
  *runtime* `PUBLIC_RUM_SAMPLE_RATE`, so it touches the runtime config only
  for the sample rate.
- **`tests/csp.spec.ts`:** both add cases; there's no shared fixture.
- **Observability spec:** this change ADDS requirements and MODIFIES only
  "Every request is traceable" and "Observability tools are restricted to the
  operator". If the business-metrics change also modifies either, the second
  to archive merges by hand.
- **`telemetry/metrics.rs`:** this change puts its instruments in the new
  `client_reports.rs` and doesn't edit `metrics.rs`, beyond a `pub` on a
  shared helper if needed.
- **OT-005's `page-view` events vs. this change's `browser.page_load` counter:**
  they measure different things for different purposes. The Frontend
  dashboard's description says `browser.page_load` is an error-rate
  denominator, not traffic analytics.

## Risks / Trade-offs

- **[Scrub misses a token format]** (e.g. a token in a path segment, not a
  query) → checked during planning: the magic link puts its token in the
  fragment (`/verify-magic#token=…`), and the reset and verify pages put the
  email in the query string. Both are covered by stripping query and fragment. Server-side scrubbing also runs OT-001's field-name redaction. The
  shared fixtures gain a case for every new link format.
- **[Browser extensions and third-party script errors create noise]** →
  errors whose top frame isn't same-origin, and `Script error.` with no stack,
  are counted with `kind: error` and `source: external`. The error-rate alert
  counts only same-origin errors.
- **[Error before the reporter loads]** → accepted. The reporter is the first
  bundled script, and islands hydrate after it.
- **[LCP, FCP and TTFB exist only for full loads]** → in-site navigations are
  covered by their own total/phase timings and per-panel ready times, which
  answer "how long until the new page was usable" more directly. Revisit when
  the soft-navigation API is stable.
- **[An Astro upgrade changes router event order or the `viewTransition`
  object]** → a Playwright test in Chromium navigates Home → Settings,
  Settings → Admin interrupted, and back. It asserts the reported phases are
  present and ordered and that the labels are right. Run it on every Astro bump.
- **[A panel forgets `markReady()`, or calls it before showing content]** →
  each panel's Vitest test asserts `markReady()` is called exactly once for
  each of content, empty and error states. The allowlist test catches an
  unwrapped island.
- **[Our own INP and CLS drift from the standard definition]** → unit tests
  feed recorded entry sequences, and for a full load with no swaps the results
  must match `web-vitals`' values for the same entries.
- **[A forged beacon under a Member's session adds noise to their user_id]** →
  no state change and bounded by the budget. Accepted; recorded in Decision 3.
- **[Endpoint shares backend capacity]** → the budget and the 204-without-work
  path keep cost small. Move it to its own service if it shows up in p95 or
  CPU; Decision 2 keeps that a deployment change.
- **[Source maps lost for old releases]** → artifacts are kept 90 days, longer
  than the 30-day log retention, so any stack in telemetry can still be
  decoded.

## Migration Plan

No schema change. Each slice deploys on its own:

- the endpoint can ship before the reporter (unused);
- the reporter can be turned off by deploying without the script, or vitals
  can be turned off with `PUBLIC_RUM_SAMPLE_RATE=0` at runtime.

Rollback is redeploying the previous image digests. No production deploy
exists yet. The Cloudflare rate-limit rule and the production alert check are
recorded for the first deploy.

## Open Questions

- **Alert threshold tuning:** whether 2% is right for browser errors with
  extension noise. Tune after real traffic; it doesn't change slices.
- **Vitals sample rate:** whether to lower it once traffic grows. It's a
  runtime setting.
- **Planned slices** (revisit after slice 1):
  1. uncaught errors end to end, with scrubbing, attribution and source maps;
  2. panel boundaries (`TelemetryBoundary` with names, without timing), API
     failure reports with trace links, page-load counter, error dashboard and
     alert;
  3. Web Vitals (full-load metrics plus per-visit INP and CLS), sampling,
     vitals dashboard panels and alert;
  4. in-site navigation and panel timing (Decisions 6a and 6b), `markReady()`
     in every panel, navigation and panel dashboard panels, and the
     slow-navigation alert.
