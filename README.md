# Rust + Svelte starter

A full-stack starting point with a finished auth slice, so a new project starts
at its first real feature instead of at login.

- **Backend:** Rust 2024, Axum 0.8, sqlx + Postgres 18, hexagonal layout
  ([backend/README.md](backend/README.md))
- **Frontend:** Astro 7 (static) with Svelte 5 islands, Tailwind 4,
  shadcn-svelte (`apps/web`)
- **Auth:** email + password (Argon2id), email verification, password reset,
  Google OAuth, Turnstile, RS256 JWT + rotating refresh tokens, CSRF, roles,
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
project, DB name, JWT issuer and audience in `backend/src/infrastructure/api/jwt.rs`,
`APP_NAME`, page titles, the leader-lock key in `main.rs`).

```bash
grep -rIl --exclude-dir={node_modules,target,.sqlx,dist} -e '\bapp\b' -e '@overkill-todo/' -e 'App\b' .
```

## Daily use

Prerequisites: [mise](https://mise.jdx.dev) and Docker. mise installs everything
else, Rust included (it installs rustup if needed), and `just` recipes always use
mise's tools, so a system Rust or Node on `PATH` doesn't matter.

```bash
mise trust && mise install   # once: just, node, pnpm, Rust, cargo tools
just setup   # once: deps, browsers, backend/.env, database
just dev     # backend :4322 + web :4321 (open this one; it proxies /api)
just         # list every recipe
just ci      # everything CI checks, locally
```

Make yourself an admin after signing up: `just grant-admin you@example.com`.
In dev, verification codes print in the backend log.

| Recipe                                | What it runs                                    |
| ------------------------------------- | ----------------------------------------------- |
| `just test` / `just test-integration` | backend unit / DB-backed tests                  |
| `just test-web`                       | `astro check` + Vitest (real Chromium)          |
| `just e2e`                            | Playwright against the built site, API mocked   |
| `just e2e-integration`                | Playwright signup against `just dev`            |
| `just smoke <url>`                    | read-only checks against a deployed site        |
| `just up`                             | prod-like stack in Docker behind Caddy on :8080 |
| `just bench` / `just bench-db`        | backend criterion benches / full-router + DB    |
| `just bench-web`                      | Vitest benches (helpers, client, AdminPanel)    |
| `just bench-compare [base]`           | base vs working tree, per-bench change report   |
| `just api-types`                      | regenerate `backend/openapi.json` + web types   |
| `just api-types-check`                | fail if either generated API file is stale      |
| `just load <scenario>`                | k6 load scenario against the capped stack       |
| `just obs-up` / `just obs-down`       | traces, logs, metrics, alerts and Grafana       |

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

- `backend/benches/crypto.rs`: JWT, CSRF, OAuth state, secret cipher, tokens,
  and Argon2 at production cost.
- `backend/benches/domain.rs`: value objects, request DTOs, error fingerprints
  and responses, email templates.
- `backend/benches/middleware.rs`: the real router's middleware stack, no DB.
- `backend/benches/http.rs` (`just bench-db`): `/me`, export, admin list,
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
  clears the test Members' refresh tokens, and waits until the stack has been
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
  `GRAFANA_ADMIN_PASSWORD`). The Service overview and Jobs dashboards are
  provisioned. Every API response carries `X-Trace-Id`; paste it into Explore →
  Tempo to open the trace, and follow the links between a trace and its logs.
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
