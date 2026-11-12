# Design

## Context

See proposal.md for the motivation and specs/analytics/spec.md for the
required behaviour. Current state that shapes the approach:

- The site is single-origin behind an edge proxy: `/api/*` → backend,
  everything else → web. Today that edge is Caddy (`deploy/compose/Caddyfile`,
  `Caddyfile.local` for the `full` stack; k8s mirrors it in
  `deploy/k8s/base/ingress.yaml`). Production isn't set up yet and will use
  **Traefik** as its edge. Cloudflare is expected in front and injects its own
  Web Analytics beacon.
- The web container's CSP (`apps/web/Caddyfile`) is `script-src 'self'` plus
  pinned hashes, `connect-src 'self'`, guarded by `tests/csp.spec.ts`.
  Same-origin scripts go in `public/scripts/` and are loaded with
  `<script is:inline src>`, so they need no hash.
- Runtime config is `window.__PUBLIC_CONFIG`, rendered from `PUBLIC_*` env by
  `docker-entrypoint.d/40-render-public-config.sh` and read via `src/lib/config.ts`.
- The Caddy edge sets no `trusted_proxies`, so behind a CDN its idea of the
  client address would be the CDN's address.
- Operator-only UIs (Grafana) bind to the host's loopback in production and
  are reached with `ssh -L`. Scheduled side jobs in compose (`backup`) are a
  `postgres:18` container running a shell loop.
- No production deploy exists yet.

No backend code changes. Everything is in `apps/web`, the edge config and the
deploy config.

## Goals / Non-Goals

**Goals:**
- One self-hosted analytics service, isolated from the product database, that
  can be dropped or wiped without affecting the app.
- No new third-party origin and no new CSP hash.
- A typed, closed event API in the frontend so free text can't be sent by
  accident.

- An edge contract (D3, D4) that any proxy can meet, so moving from Caddy to
  Traefik changes config, not design.

**Non-Goals:**
- Writing the Traefik production config. That belongs to the production
  setup. This change states what it must do and gives the Traefik mapping.
- High availability for analytics. Losing a few hours of page views is fine.
- k8s manifests in the first slice (compose is the deploy target; k8s is a later
  slice).
- Server-side event tracking from the backend.

## Hypothesis

We believe that serving Umami's tracker and collect endpoint under `/stats/`
on our own origin, with the edge passing the real client address, gives
correct distinct-Visitor counts without CSP changes or `/api` interference.
Slice 1 tests this in the `full` local stack: a Playwright test under the real
CSP sees the tracker load and a page view arrive with no CSP violations, and a
scripted check sends collect requests through the edge with two different
client addresses (via a trusted `X-Forwarded-For`) and sees 2 sessions in
Umami. Production (Traefik, behind Cloudflare) is verified on first deploy in
two ways: re-run the same check against it, and after a day compare Umami's
visitor count with Cloudflare Web Analytics. They should be within ~20%.

## Decisions

### D1. Umami, self-hosted

Umami: cookie-free, page views plus custom events and funnels, Postgres only,
~150–250 MB RAM, one container. The same choice was made for Toutouwai.
Image pinned by digest like the observability images.
- Plausible: similar features, but self-hosting needs ClickHouse (~1 GB+ idle).
- Matomo: PHP + MySQL and cookies by default.
- PostHog: per-user product analytics we don't want here (and heavy to
  self-host). Cloud options send visitor data to a third party.
- Cloudflare Web Analytics: already present, no custom events. Kept for
  comparison, not replaced.

### D2. Its own Postgres container, outside backups

A new `analytics-db` service (`postgres:18`) with its own volume and
credentials, under a new compose profile `analytics` (like `observability`).
It is not added to the `backup` service. Analytics history is replaceable and
the spec keeps it apart from account data.
- A second database on the product Postgres was rejected: shared connection
  limits and resource use, and it would be inside the product backups.

### D3. Edge contract: same-origin routing with an allowlist

Whatever the edge proxy is, it MUST:
1. Route only two paths to Umami, with the `/stats` prefix removed:
   `/stats/script.js` → `/script.js` and `/stats/api/send` → `/api/send`.
2. Answer 404 for anything else under `/stats/`, so Umami's login and reports
   are never public.
3. Leave `/api/*` and all other routes exactly as they are.

The tracker is told `data-host-url="<origin>/stats"`, so it posts to
`/stats/api/send` and never touches the backend's `/api/*`.

- **Caddy (now):** a `handle /stats/*` block ahead of the catch-all, with
  `uri strip_prefix /stats`, a matcher for the two paths and `respond 404`
  for the rest.
- **Traefik (production, later):** one router with
  ``PathPrefix(`/stats/`) && (Path(`/stats/script.js`) || Path(`/stats/api/send`))``
  and a `stripPrefix` middleware to the Umami service, at a higher priority
  than the web catch-all, plus a second `PathPrefix(`/stats/`)` router to a
  404 (or just no route, which Traefik answers with 404).
- Exposing Umami on a subdomain was rejected: it's a new origin in
  `script-src`/`connect-src` and more DNS/TLS.
- Renaming the script and endpoint (Umami's `TRACKER_SCRIPT_NAME` /
  `COLLECT_API_ENDPOINT`) to dodge blocklists is deferred. `/stats/` on our own
  origin is already rarely blocked, and blocked tracking is acceptable.

### D4. Edge contract: the real client address, trusted only from known proxies

Umami hashes the client address (with site ID, user-agent and a monthly salt)
to tell Visitors apart and derives an approximate location from it. It does
not store the address. If Umami sees a proxy's address instead, Visitors with
the same browser collapse into one session, funnels join different people's
steps together, and locations are wrong.

Whatever the edge proxy is, it MUST:
1. Work out the client address, believing forwarded-address headers
   (`CF-Connecting-IP`, `X-Forwarded-For`) only when the request comes from a
   known upstream proxy (the CDN's published ranges). Otherwise it uses the
   connection's address.
2. Pass that address to Umami in a header Umami is configured to read
   (`CLIENT_IP_HEADER`), with any copy of that header sent by the client
   removed or overwritten.

Trusting forwarded headers from anyone was rejected: anyone could inflate
visitor counts or skew locations at will.

- **Caddy (now):** global `servers { trusted_proxies static <ranges>;
  client_ip_headers CF-Connecting-IP X-Forwarded-For }`, then on `/stats/*`
  `header_up X-Analytics-Client-IP {client_ip}`. Umami reads
  `x-analytics-client-ip`. Locally the trusted ranges are the private ranges,
  so the hypothesis check can supply addresses. This also gives real client
  IPs in Caddy's access logs.
- **Traefik (production, later):** the entrypoint's
  `forwardedHeaders.trustedIPs` set to the CDN ranges, so Traefik keeps
  `X-Forwarded-For` only from them and otherwise replaces it. Umami then reads
  the first `X-Forwarded-For` entry (`CLIENT_IP_HEADER=x-forwarded-for`). If
  Cloudflare is in front, either also trust `CF-Connecting-IP` only via a
  Cloudflare-aware plugin, or firewall the origin to Cloudflare's ranges so
  every request carrying it really came through Cloudflare. Which one is
  chosen with the production setup. The hypothesis check script (task 1.6)
  is edge-agnostic and is re-run against Traefik to confirm.
- The CDN's IP ranges change rarely. They live in one config snippet with
  their source URL and a date.

### D5. Loader script and typed `track()`

- `public/scripts/analytics.js`, loaded from `Base.astro` with
  `<script is:inline src defer>` (same pattern as `theme-init.js`, no hash).
  It reads `UMAMI_WEBSITE_ID` from `window.__PUBLIC_CONFIG` and returns
  without doing anything if the ID is unset or a placeholder, or if
  `navigator.doNotTrack === '1'` or `navigator.globalPrivacyControl === true`.
  Otherwise it appends `<script defer src="/stats/script.js"
  data-website-id data-host-url data-exclude-search="true"
  data-exclude-hash="true" data-do-not-track="true">`. Umami's own DNT check
  does not cover GPC, hence our check first.
- Umami's tracker follows `history.pushState`, so Astro view-transition moves
  between dashboard pages count as page views with no extra code. This is
  verified by a test.
- `src/lib/analytics.ts` exports `track(event, props?)`, where `event` and
  `props` are a TypeScript discriminated union generated from the spec's table,
  so a free-text value fails `tsc`. It calls `window.umami?.track` inside
  `try/catch` and never awaits or throws, which keeps form submits from
  waiting on analytics.
- `config.ts` gains `UMAMI_WEBSITE_ID` and the entrypoint renders
  `PUBLIC_UMAMI_WEBSITE_ID`. Unset means off, which is the feature flag.
- Rejected: a Svelte island or an Astro-processed `<script>`. Astro may inline
  small processed scripts, which would need a CSP hash per build.

### D6. Where events are recorded

In the existing Svelte forms, after the API call settles: `RegisterForm`
(`signup-submitted` / `signup-rejected`), `VerifyEmailForm` and
`VerifyMagicForm` (`email-verified`), `LoginForm` (`signin-*`), `OAuthButtons`
(`oauth-started`, before navigation), `ForgotPasswordForm`,
`ResetPasswordForm` and `SettingsPanel` (deletion). `reason` is mapped from
HTTP status/`error_code` to the fixed list in one function in
`lib/analytics.ts`: 400/422 → `invalid`, a captcha error code → `captcha`,
429 → `rate-limited`, 401 → `credentials`, unverified → `unverified`,
network/5xx → `server`.

### D7. Operator access

Umami binds to `127.0.0.1:3001` on the host in production (`ssh -L
3001:localhost:3001 <host>`), and to `localhost:3001` locally. Its default
`admin` / `umami` login is changed on first start (a task, documented in the
README). `APP_SECRET` and the analytics DB password are SOPS secrets created
with `just secrets-new` and decrypted into `deploy/compose/.secrets/`, like
`db_password`.

### D8. Retention job and its alert

Umami OSS has no automatic retention (confirm on the pinned version and
use it instead if present). An `analytics-retention` compose service, built
like `backup`, deletes Umami event and event-data rows older than 90 days and
then sessions with no remaining events, in one transaction, once a day. After
a successful run it pushes `analytics_retention_last_success_timestamp_seconds`
to VictoriaMetrics' import endpoint. A vmalert rule in
`deploy/observability/alerts/overkill-todo.yml` (with a promtool-style test
in `alerts/tests/`) fires when `time() -` that metric exceeds 2 d 15 m,
matching the existing "job not running" rule shape. Column names are taken
from the pinned Umami schema and checked by running the job against a seeded
database in slice 2.
- Running retention as a backend job under the leader lock was rejected: the
  backend would then need credentials for the analytics database, and D2
  exists to keep them apart.

## Risks / Trade-offs

- [Edge misconfigured, so Visitors share the proxy's address] → D4 plus the
  edge-agnostic hypothesis check (re-run when Traefik replaces Caddy), and the
  first-deploy comparison with Cloudflare Web Analytics.
- [Traefik migration drops the `/stats` rules or the client-address handling]
  → The edge contract is documented in `deploy/compose/README.md` (task 1.8)
  and the hypothesis check fails if either is missing.
- [Collect endpoint is public, so anyone can post fake events] → Accepted.
  Umami drops bots and unknown website IDs. The numbers are for trends, not
  billing. Add an edge rate limit if spam appears.
- [`oauth-started` lost when the page navigates away] → Umami sends with
  `keepalive`. If the test shows loss, delay navigation by at most 150 ms,
  never longer.
- [Umami upgrade changes tracker attributes or schema] → Digest-pinned image.
  The retention SQL and the Playwright test catch drift on upgrade.
- [Session hash is monthly, not daily] → Disclosed in the spec. There is still
  no cookie and no stored IP.
- [Default Umami admin password] → Loopback-only binding plus a first-start
  task to change it.

## Migration Plan

No product schema change. Umami runs its own migrations in its own database.
Rollout: deploy `analytics-db`, `umami` and `analytics-retention`, create the
site in Umami, then set `PUBLIC_UMAMI_WEBSITE_ID` on `web` and restart it.
Rollback: unset `PUBLIC_UMAMI_WEBSITE_ID` (the site immediately stops loading
the tracker), then stop the analytics services. The `/stats/*` routes can stay
because they 404 or 502 harmlessly.

## Open Questions

- Rename the tracker script and endpoint for ad blockers (D3)? Decide after a
  month of numbers by comparing with Cloudflare Web Analytics.
