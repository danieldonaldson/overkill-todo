# Design: OT-004 Load tests

## Context

See `proposal.md` for why and `specs/performance/spec.md` for the required
behaviour. Here is the current state that shapes the approach:

- **API surface.** Only auth, profile, admin and health routes exist
  (`backend/src/infrastructure/api/routes.rs`). There are no todo endpoints yet.
- **Sign-in cost.** Password hashing is Argon2id. It already runs on
  `spawn_blocking` (`infrastructure/services/password_hasher.rs`), so it can't
  stall the async workers, but it does compete for the blocking pool and CPU.
  Production uses `65536 KiB / t=3 / p=4` (`backend/.env.example`). The local
  `backend/.env` from `scripts/gen-dev-env.sh` uses `4096 / 1 / 1`, which is
  roughly 50× cheaper. A load run against the local env as-is would flatter
  sign-in badly.
- **Session check.** `GET /auth/me` verifies the JWT and reads the user. Profile
  and admin routes also run `require_fresh_session` (one extra DB read).
- **CSRF.** Double-submit: an `X-CSRF-Token` header must match the `csrf_token`
  cookie on state-changing requests. Login, register, refresh, logout and the
  verification routes are exempt (`middleware/csrf.rs`). Login and refresh
  require an `X-Device-Fingerprint` header, which is bound to the refresh token.
- **Sign-up.** Register creates a user without a password and sends a code. Then
  verify-email, then complete-signup sets the password. In `DEV_MODE`, addresses
  whose local part starts with `test-` (and `e2e-`, ...) always get the code
  `123456` (`services/code_generator.rs`).
- **No-op adapters.** Turnstile, email (console) and MX validation fall back to
  no-op or console adapters only when `DEV_MODE=true` (`main.rs`). The `full`
  compose profile reads `backend/.env`, which is `DEV_MODE=true` locally.
- **DB pool.** `DATABASE_MAX_CONNECTIONS` is 5 in `backend/.env.example`, the
  template production uses (the code default of 10 only applies when unset).
- **Stack.** `compose.yaml` profile `full` runs the backend, web and a Caddy edge
  on `:8080` (`deploy/compose/Caddyfile.local`), built from the release
  Dockerfiles. Profile `observability` adds VictoriaMetrics (`:8428`, OTLP
  ingest, Prometheus naming) and Grafana, with dashboards provisioned from
  `deploy/observability/grafana/dashboards/`. Backend request duration is
  already exported as `http_server_request_duration_seconds` per route template.
- **Machine.** 12 cores and 31 GB RAM, shared with the developer's desktop.

## Goals / Non-Goals

**Goals:**
- One command per scenario, with a pass/fail exit code that can later go into
  CI unchanged.
- Results comparable from run to run on this machine: fixed caps, fixed cores,
  fixed hashing cost, recorded with every result.
- Load-test code that reads like the web client's API use, so adding todo flows
  later means one new flow file plus a weight in a scenario.

**Non-Goals:**
- Absolute capacity numbers for production hardware.
- Distributed or cloud load generation.
- Load-testing the static frontend or OAuth.
- Changing backend code. Findings are recorded and turned into follow-up work.

## Hypothesis

We believe that with the backend, database, edge and load generator pinned to
separate CPU cores, and the backend and database capped, p95 latency for the
smoke and auth-mix scenarios varies by less than 10% (coefficient of variation)
across 5 back-to-back runs. That would make a ±20% regression margin against a
baseline meaningful.

Slice 1 tests this. It runs auth mix 5 times on an otherwise quiet desktop and
5 times with normal desktop use (browser and editor open), then records the
per-route p95 spread here.

If variance is higher than 10%, the options in order are: lengthen the
measured window, raise the warm-up, then move to comparing two builds
back-to-back in the same session (an A/B run of `main` versus the branch, using
a `just wt-new` worktree) instead of against a stored baseline. The last option
would change Decision 10.

**Slice 1, first measurements (2026-11-12, quiet desktop, auth mix at 10 it/s,
with the first, sibling-sharing core layout; see Decision 3):**

Seven complete runs back to back on the same seeded data. The p95 values in ms
(the first two runs came before the five-run variance set):

| Run start (UTC) | me | login | logout | refresh |
|---|---|---|---|---|
| 09:34 | 1.25 | 101.4 | 3.21 | 5.35 |
| 09:40 | 1.56 | 101.5 | 3.32 | 5.61 |
| 09:48 | 1.19 | 101.9 | 3.31 | 5.39 |
| 09:51 | 1.38 | 103.9 | 7.62 | 9.62 |
| 09:55 | 1.45 | 104.7 | 8.06 | 9.73 |
| 09:58 | 1.62 | 108.2 | 9.25 | 12.74 |
| 10:02 | 1.76 | 108.2 | 10.35 | 12.37 |

Over the five-run set, the coefficient of variation of p95 was: login 2.6%,
me 15%, refresh 30%, logout 35%.

- **First reading, later corrected.** Refresh and logout p95 roughly tripled
  across back-to-back runs. A reset and reseed brought them back, so slice 1
  first blamed accumulated refresh tokens, and every run now clears them
  (Decision 7). That test was confounded, because the reseed also added a
  pause. The five runs on cleared tokens drifted the same way (first run fast,
  then slow).
- **Core layout bug.** The layout put k6 on the SMT siblings of the backend, db
  and edge cores (Decision 3). It's now fixed.

**Slice 1, physical-core layout (2026-11-12, quiet desktop, tokens cleared
before each run, 5 runs back to back), p95/p99 in ms:**

| Run start (UTC) | me | login | logout | refresh |
|---|---|---|---|---|
| 10:47 (after idle) | 1.48/2.24 | 102.4/109.5 | 2.95/4.70 | 6.43/7.93 |
| 10:50 | 1.52/2.59 | 110.8/155.0 | 7.42/16.67 | 9.48/19.53 |
| 10:54 | 1.50/2.41 | 106.8/132.5 | 7.39/17.28 | 9.42/15.62 |
| 10:57 | 1.52/2.43 | 109.0/120.7 | 8.09/19.26 | 11.11/20.23 |
| 11:01 | 1.50/2.28 | 111.2/127.6 | 8.27/19.24 | 11.34/21.16 |

Coefficient of variation of p95: me 1.1% (was 15% with the shared layout),
login 3.3%, refresh 20.5%, logout 32.3%. Among runs 2–5 only: refresh ≈ 8%,
logout ≈ 5%.

- **Reads are now stable** enough for a stored baseline. The Hypothesis holds
  for them.
- **The write path degrades under sustained load, unevenly.** k6's live
  per-interval p95 in VictoriaMetrics shows logout at about 3 ms from 10:47
  until a one-minute spike at 10:54:00–10:55:00, about 3 ms again until
  10:57:30, then 7–10 ms until the set ended at 11:05. So it isn't a fixed
  warm-up effect: the slow state arrives unevenly after roughly 7–10 minutes of
  sustained load, and an idle period clears it. A run's p95 jumps as soon as a
  spike covers more than about 5% of its measured window (run 2 caught only the
  last 17 s of one and reported 7.4 ms).

  It is not explained by checkpoints (logged at 10:52, 10:57 and 11:02; the
  10:52 one caused no spike) or by refresh-token count (cleared before each
  run). Sign-in also rises about 8% in the slow state. **Follow-up for the
  backend:** find what builds up under sustained writes. Candidates: `users`
  row updates from the `last_active_at` trigger (every token insert updates
  one of 500 rows), autovacuum timing, and the backend allocator. The slice 2
  soak scenario is built to catch exactly this.
**Slice 1, final protocol (2026-11-12, quiet desktop, physical-core layout,
tokens cleared and 120 s idle before each run, 3 runs back to back), p95/p99
mean in ms with the coefficient of variation of p95:**

| Route | p95 mean | p95 CV | p99 mean |
|---|---|---|---|
| `GET /auth/me` | 1.45 | 0.7% | 2.09 |
| `POST /auth/login` | 102.9 | 1.8% | 112.5 |
| `POST /auth/logout` | 2.98 | 4.4% | 4.66 |
| `POST /auth/refresh` | 6.17 | 5.3% | 7.46 |

**On a quiet desktop, the Hypothesis holds:** every route is under 10% CV, so
stored baselines (Decision 10) stand and the A/B fallback isn't needed. The
`auth-mix` baseline is the last of these runs.

- **Budgets** come from the worst run (Decision 5), so both states pass.
- **Not yet measured:** the "normal desktop use" set. The developer runs it
  (task 1.15).

## Outcome

Closed on 2026-11-12 after slice 1. Slice 1 delivered `smoke` and `auth-mix`
with seeding, budgets, baselines, interrupts and live metrics pushed to
VictoriaMetrics. The archived spec covers only that. Descoped to follow-up
work, with the decisions above kept as the starting design:

- The `read-baseline`, `register-burst`, `breakpoint` and `soak` scenarios
  (Decisions 4, 10 and 11). The sign-up budgets in `budgets.js` are still
  provisional, because `register-burst` was going to measure them.
- The provisioned Grafana load-test dashboard (Decision 9). k6 metrics already
  reach VictoriaMetrics when the observability stack is up.
- The busy-desktop variance set (task 1.15). Stored baselines are only known
  to hold on a quiet desktop. Re-check them before acting on a regression
  measured while the machine is in use.
- The backend follow-up from Hypothesis: find what builds up under sustained
  writes (logout/refresh p95 slowing after about 7–10 minutes of load).

## Decisions

### 1. k6 as the load generator

k6 runs multi-step scripted flows (sign in → act → refresh → sign out) with
per-Member cookie jars. It has open-model arrival-rate executors, thresholds
that set the exit code, a `handleSummary` hook for saved results, and
Prometheus remote-write output that VictoriaMetrics accepts directly. Scripts
are JavaScript, the same language as the web client and its Playwright tests.

Alternatives considered:
- **oha / vegeta**: excellent for hitting one endpoint at a fixed rate, but they
  can't do flows with cookies and CSRF. We may still use them ad hoc. They're
  not part of this change.
- **Goose (Rust)**: same language as the backend, but a smaller ecosystem, no
  built-in Prometheus output, and a compile step for every script edit.
- **Locust / Artillery / Gatling**: these need a Python, Node or JVM runtime
  that is heavier per request than k6's Go runtime, which matters when the
  generator shares the machine. They offer nothing k6 lacks for this use.

### 2. k6 runs in a digest-pinned container on the compose network

A `k6` service is added under a new `loadtest` profile in
`loadtest/compose.load.yaml`, with image `grafana/k6:2.3.0` pinned by digest like
the other images. It mounts `loadtest/` read-only plus a writable `results/`
directory. It targets `http://edge:8080`, so every request goes through Caddy
as in production (spec: scenarios don't bypass the edge). When the
observability profile is up, it reaches `victoriametrics:8428` by name.

Alternative: a host-installed k6 via mise hitting `localhost:8080`. That's
simpler to debug, but it can't be CPU-pinned as cleanly, and it would need
`localhost` port juggling per worktree. Rejected.

### 3. Compose override for caps, cores and prod-equivalent hashing

`loadtest/compose.load.yaml` is layered on top of `compose.yaml` with `-f` and
is only used by the load recipes. It sets:

| Service | Physical core | `cpuset` (default) | `cpus` | `mem_limit` |
|---|---|---|---|---|
| backend | 0 | `0,6` | 2 | 1g |
| db | 1 | `1,7` | 2 | 2g |
| edge | 2 | `2,8` | 1 | 256m |
| k6 | 3–4 | `3,4,9,10` | — | 2g |

The machine is a Ryzen 5 3600: 6 cores, 12 threads, and logical CPUs N and N+6
are SMT siblings. Each service therefore gets whole physical cores. Core 5
(`5,11`) is left for the desktop, the OS and the observability stack. Each
`cpuset` comes from an env var (`LOAD_CPUSET_BACKEND`, ...), so a different
machine can override it, keeping both threads of a core in the same service.

The first layout (backend `0-1`, db `2-3`, edge `4`, k6 `6-11`) assumed 12
separate cores. In fact it put k6 on the sibling threads of the backend, db
and edge cores, so the generator competed with what it was measuring. Found
during slice 1; the runs before the fix are marked as such under Hypothesis.

For the backend it also sets `ARGON2_MEMORY_COST=65536`, `ARGON2_TIME_COST=3`
and `ARGON2_PARALLELISM=4`, which override `backend/.env` because compose
`environment` wins over `env_file`. `DATABASE_MAX_CONNECTIONS` is left at
the `.env` value (5), the same as production. `DEV_MODE` stays `true`. It is
needed for the no-op Turnstile and MX adapters, console email and the fixed test
code.

The known differences from production while `DEV_MODE=true` are: console email
writes to stderr instead of SMTP, error bodies are verbose, and Turnstile and MX
checks are skipped. These make sign-up slightly cheaper than in production. That
is acceptable and is printed in every summary.

New recipes: `just load-up` (build and start `full` with the override and wait
for `/api/v1/health/ready` through the edge) and `just load-down`.

### 4. Scenarios, executors and rates

All scenarios use arrival-rate executors (`constant-arrival-rate` or
`ramping-arrival-rate`). Each scenario has a threshold of `dropped_iterations`
count == 0, which implements "generator cannot keep up". The rates below are
starting points, to be revised from the slice 1 baseline:

| Scenario | Executor | Shape |
|---|---|---|
| `smoke` | constant 1 it/s | 30 s; one pass through every route |
| `read-baseline` | constant | 200 it/s `GET /auth/me` + 20 it/s `/health/ready`, 2 min after 30 s warm-up |
| `auth-mix` | constant | 10 it/s; iteration = login → me ×5 → refresh → me ×2 → logout; 3 min after 30 s warm-up |
| `register-burst` | ramping | 0 → 20 sign-ups/s in 10 s, hold 30 s, back to 0 |
| `breakpoint` | ramping, stepped | auth-mix iteration from 2 it/s in steps of +2 every 60 s up to 60 |
| `soak` | constant | 50% of the last breakpoint result (default 5 it/s auth mix + 100 it/s me), 35 min (5 min warm-up) |

Warm-up is a separate k6 scenario tagged `phase:warmup`. Thresholds only select
`phase:measure`, so connection setup and cold caches don't count.

### 5. Request naming, budgets and what counts as an error

Every request is tagged `name: "<METHOD> <route template>"` (for example
`POST /api/v1/auth/login`), matching the backend's span and metric names, so
k6 and backend numbers line up in Grafana.

Budgets live in one file, `loadtest/lib/budgets.js`, as
`{ route: { p95, p99 } }` plus a per-scenario error-rate limit. Thresholds are
generated from it, so a scenario can't silently skip a limit. These provisional
values are replaced in slice 1 by max(1.5 × the worst p95 across the variance
runs, 25 ms) for p95, and max(1.5 × the worst p99, 50 ms) for p99, rounded up.
The floor exists because sub-millisecond routes swing ±15–40% between runs
(slice 1), so a plain multiple would fail on noise alone. Provisional values:

| Route | p95 | p99 |
|---|---|---|
| `GET /api/v1/auth/me` | 50 ms | 150 ms |
| `GET /api/v1/health/ready` | 30 ms | 100 ms |
| `POST /api/v1/auth/login` | 400 ms | 800 ms |
| `POST /api/v1/auth/refresh` | 100 ms | 250 ms |
| `POST /api/v1/auth/logout` | 100 ms | 250 ms |
| sign-up steps | 400 ms | 800 ms |

Error rate: 0% for smoke, under 0.1% for read baseline, under 1% for the rest.

Errors are defined with `http.setResponseCallback(http.expectedStatuses(...))`
per request where a non-2xx status is expected. `http_req_failed` then counts
only real failures: 5xx, timeouts, connection errors and unexpected statuses.

### 6. Client behaviour in a shared helper

`loadtest/lib/client.js` wraps k6 `http`. It holds the per-VU state (cookie jar,
CSRF token from the login and refresh response bodies, and a device fingerprint
generated once per VU) and adds `X-CSRF-Token` and `X-Device-Fingerprint`
where the web client would. Flows (`loadtest/flows/auth.js`, later
`flows/todos.js`) call the helper. Scenarios (`loadtest/scenarios/*.js`) only
compose flows with weights and rates. Seeded Member `i` is used by VU
`i mod N`, so each VU keeps its own session.

### 7. Seeding through the API with the dev test code

`just load-seed <n>` (default 500) runs `loadtest/seed.js` in the same k6
container (no thresholds, shared-iterations). For each `i`, it uses
`test-load-<i>@example.test` (a reserved TLD and a `test-` prefix, so the dev
code generator returns `123456`) and a fixed test password. It first tries to
sign in and skips the account on success. Otherwise it runs register →
verify-email with `123456` → complete-signup. Register already reuses an
incomplete signup, which makes partial seeds resumable.

At this cost, seeding 500 accounts takes about a minute.

Before every scenario run (not seeding), `loadtest/run.sh` deletes the test
Members' refresh tokens and runs `VACUUM (ANALYZE) refresh_tokens`. It takes
seconds and keeps the seeded accounts. Slice 1 showed revoked tokens piling up
per Member and roughly tripling refresh and logout p95 within seven runs, so
without this, runs aren't comparable. A full reset and reseed (about a minute
per run) and a manual reset with a drift warning were considered. The developer
chose the automatic token clear.

`loadtest/run.sh` also waits until `LOAD_IDLE_SECONDS` (default 120) have
passed since the previous load run ended, which it records in
`loadtest/results/.last-load-end`. Slice 1 showed the write path drifting into
a slower state after about 7–10 minutes of sustained load, and an idle period
clearing it (Hypothesis). With the wait, every 3.5-minute run starts clean and
stays well before that point. Considered instead: a 10-minute warm-up to
measure the slow state, at about 13.5 minutes per run. The developer chose the
clean state. The slow state belongs to the soak scenario (slice 2) and a
backend follow-up.

Every scenario's `setup()` signs in as Member `N-1` (`N` from `LOAD_USERS`, the
same default). If that fails, it aborts with "run `just load-seed` first", which
implements the spec's "scenario run without seeding".

Alternative: insert rows with SQL and a precomputed hash. That's faster, but it
skips the real sign-up path and couples the load tests to the schema. Rejected.

### 8. Local-only guard

The target comes from `LOAD_BASE_URL` (default `http://edge:8080`). `lib/config.js`
aborts in the init context unless the host is `edge`, `localhost` or `127.0.0.1`,
or `LOAD_ALLOW_REMOTE=1` is set. The recipe also checks
`/api/v1/health/ready` through the edge within 10 s before starting k6 (spec:
stack not running).

### 9. Live metrics to VictoriaMetrics, plus a Grafana dashboard

When `victoriametrics` resolves on the compose network, the recipe adds
`--out experimental-prometheus-rw` with
`K6_PROMETHEUS_RW_SERVER_URL=http://victoriametrics:8428/api/v1/write` and
`K6_PROMETHEUS_RW_TREND_STATS=p(95),p(99),avg`. Otherwise it runs without
output (spec: observability down). Every series carries a `testid` tag (the
scenario plus a timestamp).

A provisioned dashboard, `deploy/observability/grafana/dashboards/loadtest.json`,
shows the k6 rate, p95, errors and dropped iterations next to the backend's
`http_server_request_duration_seconds` p95 per `http_route`, the DB pool gauges
and the backend container CPU, with a `testid` variable. It is local-only in
practice: production never receives k6 metrics.

### 10. Summaries, baselines and post-run checks

`handleSummary` writes `loadtest/results/<scenario>-<UTC timestamp>.json`
(gitignored), containing the k6 summary data plus metadata passed in by the
recipe: commit, scenario, caps, cpusets, Argon2 params, `DEV_MODE`, and whether
the run completed its planned duration.

`loadtest/report.mjs` (run with the repo's mise-managed Node after k6 exits)
does what k6 thresholds can't. It compares against
`loadtest/baselines/<scenario>.json`, checks soak drift (it compares the
`window:early` and `window:late` tagged trends, failing above 20%), and works
out the breakpoint (Decision 11). The recipe exits non-zero if either k6 or
`report.mjs` fails.

Baselines are committed. They are machine-specific, but this is a solo project
on one machine, and git history then shows performance over time.
`just load-baseline <scenario>` copies the latest complete result for that
scenario, and refuses incomplete runs.

### 11. Breakpoint as tagged steps

The stepped ramp tags each plateau `step:<rate>`. Per-step thresholds are
generated for p95 and the error rate. The overall p95 and error-rate thresholds
use `abortOnFail: true, delayAbortEval: '30s'`, so a sustained breach stops the
run early. `report.mjs` reports the highest step whose own thresholds passed,
and which limit failed at the next step.

### 12. Interrupts

`loadtest/run.sh` starts `docker compose run --rm -T --name <unique> k6 ...` in
its own session (`setsid`), so the terminal's Ctrl-C never reaches Compose.
Compose would otherwise kill k6 outright, with no summary (found in slice 1).
The script's INT/TERM trap sends SIGINT to the k6 container instead
(`docker kill --signal INT`). k6 then stops, still calls `handleSummary`, and
the run is marked incomplete because its elapsed time is less than planned. A
second Ctrl-C kills the container. An EXIT trap's `docker rm -f <name>` ensures
nothing is left behind.

### 13. Not in `just ci`

The runs take minutes, need the full release stack and are noisy. `just ci` is
unchanged. The smoke scenario is built to be CI-ready (short, a 0% error budget,
no observability dependency) for a possible later change.

## Risks / Trade-offs

- [Desktop activity skews results] → Pin cores, record every run's metadata,
  measure variance in slice 1 (Hypothesis), and fall back to A/B runs in the
  same session if needed.
- [`DEV_MODE=true` differs from production] → The differences are listed in
  Decision 3 and printed in every summary. Hashing cost, the thing that matters
  most, is forced to production values.
- [Load fills the local database with test accounts and refresh tokens] → All
  test accounts share the `test-load-` prefix. `just load-reset` deletes them
  (and their tokens, by cascade) with one `psql` statement, run like
  `grant-admin`.
- [Cores 0–4 are also used by the desktop] → They are capped, not reserved.
  The cpusets can be overridden per machine. The summary records them.
- [k6 remote-write output is "experimental"] → It is used only for the live
  view. Pass/fail and summaries never depend on it.
- [Provisional budgets are guesses] → They are replaced from the slice 1
  baseline before any regression is acted on.
- [Seeding at production hashing cost is slow] → About a minute for 500, and it
  only runs once per fresh database.

## Open Questions

- Should the soak's drift limit (20%) and the breakpoint's step size be tuned
  after the first real runs? Changing them doesn't affect structure, only
  values in `budgets.js` and the scenario files.
