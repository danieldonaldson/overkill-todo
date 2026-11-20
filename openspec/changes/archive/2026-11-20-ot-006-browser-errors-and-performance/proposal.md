# OT-006: Browser errors and performance

## Why

As the operator, I want to know when the site breaks or is slow in a Visitor's
or Member's browser, so that I can fix problems that never reach the server's
logs.

OT-001 shows how the backend behaves. It can't see what happens after a page
leaves the server. A script error that kills a dashboard panel, a sign-in form
that stops responding, or a page that takes eight seconds to become usable on a
slow phone produces no backend error and no alert. Cloudflare Web Analytics
gives aggregate page-speed numbers in Cloudflare's dashboard, but not errors,
not per release, and not linked to our traces or alerts. This is the last big
blind spot in operational telemetry, and closing it before the first deploy
means the first real users are covered.

## What Changes

- The site reports uncaught script errors, failed promise handling, and errors
  inside an interactive panel to the backend. A panel that fails shows a
  fallback with a retry, not a broken or blank area.
- When an API call fails (network error or server error), the report carries
  the request's trace ID, so the operator can go from the browser error to the
  backend trace in one click.
- For a configurable sample of page loads, the site measures real loading and
  responsiveness (the standard Core Web Vitals and two supporting timings),
  every in-site navigation (time from the click to a usable page, including
  the view transition animation, and whether the animation ran, was skipped
  or isn't supported), and how long each interactive panel takes to become
  interactive and to show its content after a page load or navigation.
- Reports are anonymous for Visitors. For signed-in Members they carry the user
  ID taken from the session, never from anything the browser claims.
- Reports never contain query strings, URL fragments, email addresses, form
  content, cookies or tokens. Sending them never delays or breaks the page, and
  costs the site nothing it can notice when the backend is down.
- The operator gets a Frontend dashboard (error rate, top errors, Web Vitals,
  navigation and panel loading times per page and release) and alerts for a
  client error spike, sustained poor responsiveness, and slow in-site
  navigation.
- Minified stack traces can be turned back into source locations by the
  operator; the maps that allow this are never served publicly.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `observability`: adds browser errors and real-user performance to what the
  operator can see and be alerted on; extends the privacy rules to browser
  reports; and states that the site's browser-report endpoint is a validated
  app endpoint, not a public telemetry ingestion endpoint.
- `dashboard`: a dashboard panel that crashes is contained. It shows a fallback
  with a retry while the header, navigation and other panels keep working.

## Appetite

Three weeks.

## Out of Scope

- **Business and product metrics** (OT-005 web analytics, and the planned
  backend business counters for sign-ups, sign-in outcomes and OAuth failures).
  This change measures whether the site *works*, not how people *use* it. Its
  page-load count exists only as the denominator for error rates and is not a
  page-view report.
- Traces that start in the browser. The edge still starts every trace and
  ignores client-supplied trace context.
- Hosted or self-hosted error products (Sentry, GlitchTip, Grafana Faro).
- A separate ingest service. Kept possible, not built.
- Session replay, console capture, breadcrumbs of user actions.
- Removing Cloudflare Web Analytics; it stays as a cross-check.

## Dependencies

- **OT-005 (web analytics), in flight in parallel.** Both changes add a
  same-origin loader to the site's base layout, a `PUBLIC_*` runtime setting,
  and CSP test coverage. Whichever lands second rebases onto the first. OT-005's
  riskiest unknown (the real client address behind Cloudflare and the edge) does
  not block this change, which never uses client addresses.
- **Planned backend business-counters change.** Shares the observability spec,
  the metrics module, the Grafana dashboards folder and the alert rules file.
  Each change adds its own dashboard and alert group, so edits don't overlap.
- No production deploy exists yet. Edge rate limiting and checking alerts in
  production are recorded for the first deploy.

## Ready

- **Riskiest unknown:** whether a browser error report is both *useful* and
  *safe*: the minified stack can be decoded against source maps that are never
  served, and secrets in the address (the magic-link token in the fragment,
  email addresses in the query string) are removed from every place a URL can
  appear (message, stack, file name, page).
- **First slice:** an uncaught error on any page is reported end to end into
  the operator's log search, attributed to the Member when signed in, with the
  token and email scrubbed from the magic-link and reset pages and the stack
  decoded from a release build.

## Impact

`apps/web` (a telemetry module, base layout loader, panel error boundaries,
`HttpClient` failure hook, runtime config, build settings for hidden source
maps), `backend` (one public API endpoint with its DTO, use case and telemetry
adapter; OpenAPI and web types regenerated), `deploy/observability` (Frontend
dashboard, alert group and tests), the web image build, the justfile
(`decode-stack`), `CLAUDE.md`, `README.md`, and the observability spec.
