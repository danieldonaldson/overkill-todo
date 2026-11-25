# Rust + Svelte starter

A full-stack starting point with a finished auth slice, so a new project starts
at its first real feature instead of at login.

- **Backend:** Rust 2024, Axum 0.8, sqlx + Postgres 18, hexagonal layout
  ([backend/README.md](backend/README.md))
- **Frontend:** Astro 7 (static) with Svelte 5 islands, Tailwind 4,
  shadcn-svelte (`apps/web`)
- **Auth:** email + password (Argon2id), email verification, password reset,
  Google OAuth, Turnstile, HMAC-SHA-512 JWT + rotating refresh tokens, CSRF, roles,
  admin user list, account export and scheduled deletion
- **Tests:** unit (mockall, proptest), integration (`#[sqlx::test]`), Vitest
  browser-mode components, Playwright e2e (mocked API), real-stack e2e, smoke
- **Ops:** Docker images, Compose for local and single-host production,
  optional Kubernetes base, GitHub Actions CI, Dependabot

## Start a project from it

```bash
cp -r ~/Work/dev-workflows/starters/rust-svelte ~/Work/myproject
cd ~/Work/myproject && rm -rf node_modules backend/target && git init
```

Then rename: `app` / `@overkill-todo/*` / `App` are placeholders (package names, Compose
project, DB name, JWT issuer and audience in `backend/crates/platform/src/crypto/token.rs`,
`APP_NAME`, page titles, the leader-lock key in `backend/crates/server/src/main.rs`).

```bash
grep -rIl --exclude-dir={node_modules,target,.sqlx,dist} -e '\bapp\b' -e '@overkill-todo/' -e 'App\b' .
```

## Daily use

Prerequisites: [mise](https://mise.jdx.dev) and Docker. mise installs everything
else, Rust included (it installs rustup if needed), and `just` recipes always use
mise's tools, so a system Rust or Node on `PATH` doesn't matter.

```bash
mise trust && mise install   # once: just, node, pnpm, Rust, cargo tools
just setup   # once: deps, browsers, Kani, backend/.env, database
just dev     # backend :4322 + web :4321 (open this one; it proxies /api)
just         # list every recipe
just ci      # everything CI checks, locally
```

Make yourself an admin after signing up: `just grant-admin you@example.com`.
It records the change in the audit log as done by the operator.
In dev, verification codes print in the backend log.

Tokens are signed with HMAC-SHA-512 keys (`JWT_SIGNING_KEY`, plus
`JWT_TRUSTED_KEYS` during a rotation; steps in `backend/.env.example`). A
`backend/.env` from before OT-009 still holds `JWT_PRIVATE_KEY`/`JWT_PUBLIC_KEY`
and the backend refuses to start with them: replace both with
`JWT_SIGNING_KEY=$(just jwt-keygen dev)`, or regenerate the file with
`FORCE=1 ./scripts/gen-dev-env.sh`. Committed secrets use post-quantum age keys
only (`deploy/secrets/README.md`).

**First-deploy / OT-012 token cutover:** all four application JWT kinds
(session, signup, password-reset and OAuth state) now require `iss`, `aud` and
`iat` (plus `typ` on OAuth state). Pre-OT-012 signup, reset and OAuth-state
tokens lack those claims and are rejected after upgrade, and pre-OT-012
binaries reject post-OT-012 tokens carrying `aud`, so do a clean cutover rather
than a rolling deploy or rollback across OT-012.

| Recipe                                 | What it runs                                                       |
| -------------------------------------- | ------------------------------------------------------------------ |
| `just test` / `just test-integration`  | backend unit / DB-backed tests                                     |
| `just test-web`                        | `astro check` + Vitest (real Chromium)                             |
| `just e2e`                             | Playwright against the built site, API mocked                      |
| `just e2e-integration`                 | Playwright signup against `just dev`                               |
| `just smoke <url>`                     | read-only checks against a deployed site, incl. post-quantum TLS   |
| `just jwt-keygen [kid]`                | new token key for `JWT_SIGNING_KEY` / `JWT_TRUSTED_KEYS`           |
| `just up`                              | prod-like stack in Docker behind Caddy on :8080                    |
| `just bench` / `just bench-db`         | backend criterion benches / full-router + DB                       |
| `just bench-web`                       | Vitest benches (helpers, client, AdminPanel)                       |
| `just bench-compare [base]`            | base vs working tree, per-bench change report                      |
| `just api-types`                       | regenerate `backend/openapi.json` + web types                      |
| `just api-types-check`                 | fail if either generated API file is stale                         |
| `just fuzz-api [seed] [examples]`      | fuzz every API route against `openapi.json` on a throwaway DB      |
| `just threat-model-check`              | every auth security property names a check that still exists       |
| `just verify`                          | Kani proofs of the auth domain policy, every input within bounds   |
| `just models`                          | Quint concurrency models of code entry and refresh, replayed on DB |
| `just protocols`                       | Tamarin proofs of the sign-in protocols against a network attacker |
| `just load <scenario>`                 | k6 load scenario against the capped stack                          |
| `just decode-stack <release> [file]`   | a browser error's minified stack → source lines                    |
| `just obs-up` / `just obs-down`        | traces, logs, metrics, alerts and Grafana                          |
| `just analytics-up` / `analytics-down` | Umami web analytics on :3001                                       |

### What's proven about auth

[`docs/security/threat-model.md`](docs/security/threat-model.md) lists every
auth security property, the attackers it holds against, and the check behind
it; `just threat-model-check` keeps that list honest. The strongest checks:

- **`just verify`** (Kani, about 2 minutes) proves the domain rules for every
  input within stated bounds: the code-entry lockout, code and magic-link
  expiry, refresh-token status, role changes, the deletion lifecycle, and
  email normalisation (ASCII up to 8 bytes).
- **`just models`** (Quint, about 4 minutes) checks races between concurrent
  requests over every interleaving of up to 8 steps, and replays the models'
  traces against Postgres.
- **`just protocols`** (Tamarin, about a minute) proves sign-in, refresh, Google
  sign-in and CSRF against an attacker who controls the network.

They cover the rules and protocols as modelled, not everything: SQL around the
proven rules is tested, not proven, and the threat model lists each accepted
gap and assumption (TLS, the browser, the edge). Tools and pins are in
`.mise.toml`; details are in
[backend/ARCHITECTURE.md](backend/ARCHITECTURE.md).

### API fuzzing

`just fuzz-api` (part of `just ci`, about 20 seconds) sends every described API
operation generated requests, most of them malformed, and checks each response
against `backend/openapi.json`. It uses [Schemathesis](https://schemathesis.io),
pinned in `apifuzz/` (`uv`, from `.mise.toml`, installs it and its Python). It
creates and migrates its own database `overkill_todo_fuzz_<pid>`, starts the
backend in dev mode on `FUZZ_API_PORT` (4399), and drops both afterwards, even
on Ctrl-C. Your dev database is never touched. There are three passes: as a
Visitor, signed in (a Member, or a Super Admin on `/admin/*`), and a Member on
admin routes. A run fails on any of these:

- a 5xx;
- a status, content type or body the description doesn't document;
- malformed input not refused with the JSON 400;
- valid input refused;
- a protected route that serves a caller without credentials, or a missing
  CSRF header that doesn't get 403;
- an error body that isn't the standard shape or carries internal text.

- **Reproducing:** a run prints its seed. `just fuzz-api <seed> <examples>`
  (defaults `1 25`) sends the same requests again, and the input digest each
  pass prints shows it. Each failure includes a `curl` command, and the logs
  are in `apifuzz/out/`.
- **CI:** CI's `api-fuzz` job runs the short run, and images need it to pass.
  `fuzz-nightly.yml` runs a random seed with 500 examples every night (or by
  hand, with your own seed). It gates nothing, and its job summary has the
  command that reproduces it.
- **Excluding an operation, or one check for an operation:** add an
  `[[operations]]` entry with its reason to `apifuzz/schemathesis.toml`.
  Today `GET /auth/oauth/{provider}/callback` is excluded, because it calls
  Google. Six operations whose 400s depend on server state skip the
  "valid input is accepted" check.
- **The fuzzer's own checks** live in `apifuzz/hooks.py`, with unit tests in
  `apifuzz/tests/` (`just fuzz-api-test`).

### Benchmarks

Benchmarks catch performance regressions; they're not part of `just ci` because
wall-clock numbers on shared runners are too noisy to gate on.

On every PR, the Benchmarks workflow runs `scripts/bench-compare.sh`: it
benchmarks the base commit and the PR on the same runner, alternating sides for
two rounds and keeping each side's fastest run. The table goes in the job summary;
each bench more than 10% slower, with the two sides' confidence intervals not
overlapping, gets a warning annotation. It never fails the PR. `bench-db` isn't
included, because query latency is too noisy for this. Run the same comparison
locally with `just bench-compare` (default base `main`; `--backend` / `--web` for
one suite). Tune it with `BENCH_THRESHOLD`, `BENCH_ROUNDS` or `CRITERION_ARGS`.

For quick manual checks, compare against a saved baseline:

```bash
git switch main && just bench --save-baseline main          # once per main change
git switch my-branch && just bench --baseline main          # prints % change per bench
just bench-web --outputJson .bench/main.json                # frontend, on main
just bench-web --compare .bench/main.json                   # frontend, on your branch
```

- `backend/crates/server/benches/crypto.rs`: JWT, CSRF, OAuth state, secret cipher, tokens,
  and Argon2 at production cost.
- `backend/crates/server/benches/domain.rs`: value objects, request DTOs, error fingerprints
  and responses, email templates.
- `backend/crates/server/benches/middleware.rs`: the real router's middleware stack, no DB.
- `backend/crates/server/benches/http.rs` (`just bench-db`): `/me`, export, admin list,
  login, and refresh rotation against a throwaway `overkill_todo_bench` database.
- `apps/web/**/*.bench.ts`: Vitest benches in Chromium. Bench mode skips
  `beforeAll`, so set up in the `describe` body; pass `{ throws: true }` to async
  benches so a failure fails the run instead of reporting NaN.

Criterion HTML reports land in `backend/target/criterion/report/index.html`.

### Load tests

k6 scenarios that put repeatable load on the prod-like stack on this machine
and fail if a latency or error budget is exceeded (OT-004). Like benchmarks,
they're not part of `just ci`.

```bash
just load-up            # release images behind Caddy, pinned to cores and capped
just load-seed          # 500 verified test Members via the real sign-up flow (about 1 min, repeatable)
just load smoke         # 30 s, every route, zero errors allowed
just load auth-mix      # sign in, check session, refresh, sign out at 10/s for 3 min
just load lists         # open a list, add and tick todos, at 10/s for 3 min
just load lists-large   # the same list of 1,000 todos from several tabs, at 10/s for 3 min
just load-baseline auth-mix  # make the latest complete run the baseline (commit it)
just load-reset         # delete the test Members
just load-down
```

- **Pass/fail.** Budgets (p95/p99 per route, error rate per scenario) live in
  `loadtest/lib/budgets.js`. A run also fails if k6 couldn't start requests at
  the planned rate (`dropped iterations`), because the results would describe a
  lower rate than intended.
- **Comparable runs.** `loadtest/compose.load.yaml` pins backend, database, edge
  and k6 to separate physical cores and caps their CPU and memory. The defaults
  (`LOAD_CPUSET_BACKEND=0,6`, `_DB=1,7`, `_EDGE=2,8`, `_K6=3,4,9,10`) suit a
  6-core / 12-thread CPU where CPUs N and N+6 share a core. On another machine,
  check `/sys/devices/system/cpu/cpu*/topology/thread_siblings_list` and keep
  both threads of a core in one service. It also forces production Argon2 cost, so sign-in isn't
  flattered by the cheap local setting. Every result records these settings, and
  the report warns when a baseline was taken with different ones. Each run first
  clears the test Members' refresh tokens (a list scenario also resets their lists), and waits until the stack has been
  idle for 2 minutes since the last run (`LOAD_IDLE_SECONDS`, `0` skips). After
  7–10 minutes of continuous load, refresh and sign-out drift into a slower
  state that only an idle period clears, so back-to-back runs wouldn't be
  comparable.
- **Results** go to `loadtest/results/` (gitignored). After each run, the p95 per
  route is compared with `loadtest/baselines/<scenario>.json` if one exists.
  Ctrl-C stops a run and still prints its summary, marked incomplete; incomplete
  runs can't become baselines.
- **Live view.** With `just obs-up` running, k6 streams its metrics to
  VictoriaMetrics (tagged `testid=<scenario>-<time>`). Without it, runs behave the
  same.
- **Not production.** The stack runs with `DEV_MODE=true` (needed for the test
  verification code and the no-op Turnstile and MX checks), and email goes to the
  console. Results are relative to this machine: use them to catch regressions and
  find the first bottleneck, not as a capacity figure for the production host.
- **Local only.** Runs refuse any target but the local edge unless
  `LOAD_ALLOW_REMOTE=1` is set along with `LOAD_BASE_URL`.

Add a flow in `loadtest/flows/` and a scenario in `loadtest/scenarios/` (one file
each; the file name is the scenario name), and give every new route a budget.

### Observability

The backend exports traces and metrics over OpenTelemetry, and the collector
picks up every container's logs. They land in Tempo, VictoriaLogs and
VictoriaMetrics, with vmalert and Alertmanager for alerts and Grafana in front
(OT-001; config under `deploy/observability/`). It's an opt-in compose profile:

```bash
just up && just obs-up   # prod-like stack; its backend already exports to the collector
just obs-down
just obs-wipe            # delete stored traces, logs and metrics (keeps Grafana)
```

- **Grafana** is on http://localhost:3000 (`admin` / `admin` locally, or set
  `GRAFANA_ADMIN_PASSWORD`). The Service overview, Jobs, Auth security and
  Frontend dashboards are provisioned. Every API response carries
  `X-Trace-Id`; paste it into Explore → Tempo to open the trace, and follow
  the links between a trace and its logs.
- **Alertmanager** is on http://localhost:9093. Locally it has no receiver, so
  alerts show there and never send email.
- **With `just dev`** (backend on the host), set
  `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318` in `backend/.env`, and
  start the stack with
  `BACKEND_READY_URL=http://host.docker.internal:4322/health/ready just obs-up`
  so the collector's readiness probe (the backend-down alert) reaches it.
- **Without the stack**, leave `OTEL_EXPORTER_OTLP_ENDPOINT` empty: nothing is
  exported, but trace IDs and log correlation still work. With `DEV_MODE=false`
  the backend refuses to start without it.
- **Ports** are bound to 127.0.0.1 only (`GRAFANA_PORT`, `ALERTMANAGER_PORT`).
  `just obs-check` (part of `just ci`) validates the collector config,
  Alertmanager configs and alert-rule tests.
- **Log filtering (`RUST_LOG`).** `RUST_LOG` targets workspace crate names
  (e.g. `api=debug,application=debug,persistence_postgres=debug`) instead of
  `overkill_todo`, which now covers only `main.rs` startup and shutdown.

### Browser errors

The site reports uncaught script errors, unhandled promise rejections, panel
crashes and failed API calls from every page to `POST /api/v1/client-telemetry`
(OT-006). They show up as `client_error` events in VictoriaLogs and on the
Frontend dashboard, with the page template, release, a fingerprint, the
browser family and device class, and a signed-in Member's `user_id` (from the
session, never from the report).

- **Panels.** Every island's root component wraps its markup in
  `TelemetryBoundary` with an allowlisted `name`. A crash replaces only that
  island with a short message, "Try again" and "Reload the page", and is
  reported as `kind: boundary` with the island. A new island needs the
  boundary and its name in `backend/tests/fixtures/client_islands.json`; the
  guard test (`islands.test.ts`) and the backend's allowlist test fail
  otherwise. In `astro dev` and `just e2e` builds,
  `window.__otCrashIsland = '<name>'` crashes that island on purpose. A panel
  that loads data also creates `createIslandTiming()`, passes it as the
  boundary's `timing` and calls `timing.markReady()` once it first shows its
  content, empty state or error state (see Navigations and panels).
- **API failures.** `HttpClient` reports a 5xx or an unreachable server (never a
  4xx) as `kind: api`, with the route template (pass `route` for paths with
  parameters, e.g. `/admin/users/{id}/role`), the status and the response's
  trace ID. "View backend trace" on the report opens it in Tempo.
- **Error rate.** Each full load and in-site navigation is counted in
  `browser.page_load` (the denominator, not traffic analytics). Errors from the
  site's own scripts are counted in `browser.error`, per page, kind and release
  (at most 5 releases per replica; the rest are `other`). `BrowserErrorRateHigh`
  fires above 2% of a page's loads for 10 minutes, with at least 1 page load a
  minute on that page.
- **Web Vitals.** A share of full page loads (`PUBLIC_RUM_SAMPLE_RATE`, 0–1,
  rendered into `/config.js` at container start; unset or invalid means all)
  also reports LCP, FCP and TTFB for the full load, and INP and CLS for each
  page visit, in-site navigations included. They go to the
  `browser.web_vital` (ms) and `browser.web_vital.cls` histograms, per page,
  rating, device class and signed-in or not; poor ones are also logged as
  `client_vital_poor` with the element responsible (tag, ID and classes only).
  `BrowserVitalPoor` warns when a page's p75 INP stays over 500 ms or LCP over
  4 s for an hour, with at least 50 measurements an hour. Releases are
  compared by time: `browser.release_seen` marks each release's first page
  load on the Frontend dashboard, which compares the 24 h before and after
  the selected release. Errors and page loads are never sampled.
- **Navigations and panels.** The same sampled page loads time every
  navigation (`lib/telemetry/navigation.ts`, from the ClientRouter lifecycle):
  full loads as `hard`, in-site ones as `soft` with fetch, swap and
  view-transition phases and whether the transition ran, was skipped or is
  unsupported. A navigation interrupted by another counts as aborted for where
  it was heading. They go to `browser.navigation.duration`,
  `browser.navigation.phase.duration` and the `browser.navigation` counter
  (in-site only). Each island's boundary records when it became interactive
  and ready, from the start of the navigation that showed it, in
  `browser.island.duration`; an island with nothing to load is ready when it
  mounts, and marks from a navigation the Member already left are dropped.
  `BrowserNavigationSlow` warns when p75 in-site navigation time to a
  dashboard page stays over 1 s for an hour, with at least 50 completed
  navigations an hour.

- **Privacy.** The browser scrubs each report before sending it, and the
  backend scrubs it again: query strings and fragments are cut from every
  address, emails become `[EMAIL]`, and sensitive `key=value` pairs are
  redacted. Both copies of the rules run the cases in
  `backend/tests/fixtures/client_scrub_cases.json`; add a case for any new
  link format. Nothing is stored in the browser.
- **Limits.** The browser sends at most 20 distinct errors per page load (repeats
  are counted), in batches under 16 KB. The backend records at most
  `CLIENT_TELEMETRY_MAX_EVENTS_PER_SEC` events per second per replica (default
  200); the rest are answered 204 and counted in `browser.report.dropped`.
  The route is public, needs no CSRF token and isn't traced.
- **Release.** `PUBLIC_RELEASE` is baked in at build time: the `RELEASE` build arg
  of the web image (CI passes the commit SHA, `just up` passes `$RELEASE`), and
  `dev` otherwise.
- **Stacks.** The site's source maps are never served. A local build keeps them
  in `apps/web/dist`; the web image moves them out, and CI keeps each release's
  maps as the `web-sourcemaps-<sha>` artifact for 90 days. To read a logged stack:

  ```bash
  just decode-stack dev stack.txt         # after a local `pnpm --filter @overkill-todo/web build`
  just decode-stack <sha> < stack.txt     # fetches and caches that release's maps (needs `gh`)
  ```

### Audit log

Role changes, password changes, sets and resets, Google links and unlinks,
account deletion (requested, cancelled and carried out), data exports and the
token-reuse sign-out are recorded in `audit_events` (OT-008). Each is stored
in the same transaction as its action. Events name accounts by ID only, can't
be changed or deleted by the app, and are deleted after 7 years. Super Admins
read them at `/dashboard/audit`, and each Member sees their own under Security
activity in Settings. How it works and how to add a kind:
[`backend/ARCHITECTURE.md`](backend/ARCHITECTURE.md#audit-log).

### Web analytics

Anonymous page views and sign-up/sign-in events in a self-hosted Umami with
its own Postgres (OT-005): no cookies, no browser storage, no query strings or
fragments, nothing from
browsers sending Do Not Track or Global Privacy Control. Only
`/stats/script.js` and `/stats/api/send` go through the edge; reports are
local-only. It's off unless `PUBLIC_UMAMI_WEBSITE_ID` is set, and only works
behind the edge (`just up`), not with `just dev`.

```bash
just up                  # prod-like stack (in another terminal)
just analytics-up        # Umami on http://localhost:3001
```

1. Sign in to http://localhost:3001 as `admin` / `umami` (local default) and
   add a website (any domain). Copy its website ID.
2. Restart the web container with it:
   `PUBLIC_UMAMI_WEBSITE_ID=<id> docker compose --profile full up -d web`
   (in a worktree, load `.worktree.env` first so the project name matches).
3. Open the site through the edge (http://localhost:8080). The page view
   shows in Umami within a minute. Umami silently drops bots, headless
   browsers included, so check with a normal browser.

**Events.** The auth forms record a fixed set of named events through
`track()` in `apps/web/src/lib/analytics.ts` (`signup-submitted`,
`signup-rejected`, `email-verified`, `signin-succeeded`, `signin-rejected`,
`oauth-started`, `password-reset-requested`, `password-reset-completed`,
`account-deletion-requested`). Its type only accepts the spec's names and
values, so nothing identifying can be sent. To add an event, change the spec's
table in `openspec/` first, then the type. To see the sign-up funnel: Umami →
the website → Funnels → add steps _Path_ `/register`, _Event_
`signup-submitted`, _Event_ `email-verified`. Locally, sign up with an
`e2e-…@example.com` address: in `DEV_MODE` its code is always `123456`.

`scripts/check-analytics-visitors.sh` checks that the edge passes the real
client address (2 Visitors from 2 networks, no address stored). Unset the ID
to turn analytics off. Data is kept 90 days (the `analytics-retention`
service; `just analytics-retention-check` tests its SQL) and isn't backed up.
Production setup, the edge
contract a Traefik edge must meet, and operator access are in
[`deploy/compose/README.md`](deploy/compose/README.md#web-analytics).

## Parallel work (worktrees)

```bash
just wt-new feat/thing        # ../overkill-todo-feat-thing on a new branch off HEAD
just wt-new fix/x origin/main # or off another base; reuses the branch if it exists
just wt-list                  # every checkout and its ports
just wt-rm feat/thing         # drop the checkout + its database (keeps the branch)
```

Each worktree gets its own Postgres (Compose project `overkill-todo-<branch>`),
its own ports (slot N: web `4321+10N`, API `4322+10N`, e2e preview `4323+10N`,
DB `5432+N`, edge `8080+N`) and its own `backend/.env`. All of that lives in its
gitignored `.worktree.env`, which `just` loads, so `just dev`, `just e2e` and
`just db-migrate` there never touch another checkout's server or schema. A `.env`
symlink to it makes plain `docker compose` there use the worktree's project too
(worktrees made before that was added: `ln -s .worktree.env .env`). The main
checkout has neither file and keeps the default ports.

Each worktree has its own `backend/target`, so its first build starts cold.
`sccache` (installed by mise, wired in by the justfile) shares compiled
dependencies between them; `sccache --show-stats` shows the hit rate. Its cache
sits in `~/.cache/sccache`, capped at 10 GiB by default (`SCCACHE_CACHE_SIZE`).
Google OAuth only accepts redirect URIs you've registered, so real Google sign-in
works only on the main checkout's `:4321` (the dev-mode bypass works everywhere).

## Layout

```
backend/            Rust API (see backend/README.md, backend/ARCHITECTURE.md)
apps/web/           Astro + Svelte site, Caddy image
packages/           shared design tokens
deploy/compose/     single-host production (Caddy TLS, digests, nightly dumps)
deploy/k8s/         optional kustomize base (CNPG Postgres, Ingress)
docs/security/      auth threat model: attackers, properties, and the check for each
apifuzz/            API fuzzer (Schemathesis): config, sign-in hooks, run script
scripts/            dev env generation, worktree create/remove
.github/            CI and Dependabot
```

## Conventions

- **Commits:** Conventional Commits, enforced by commitlint (`feat:`, `fix:`,
  `chore:` …). Pre-commit hooks are path-scoped and fast, so don't skip them.
- **Branches:** short-lived branches into `main`; `main` is always deployable.
- **Dependencies:** pnpm refuses releases younger than 24h; Dependabot waits 3
  days; `cargo deny` (advisories, licenses, sources;
  also weekly) and `cargo machete` (unused deps) run in CI.
- **Database:** forward-only migrations, expand then contract; let Postgres
  enforce invariants (constraints, partial unique indexes).
- **Deploys:** an image digest change committed to the deploy config. CI builds
  images; it never holds production credentials.

## Where this came from

Extracted from a production project and trimmed to the auth core. The reasoning
behind most choices lives in the notes vault: start at _Starting a new project_.
