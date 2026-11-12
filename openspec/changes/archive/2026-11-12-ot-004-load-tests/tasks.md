# Tasks

This change is local tooling, with nothing to deploy. A slice "ships" when it's
merged to `main` with `just ci` green and checked on this machine with the load
stack up.

Only slice 1 was built. At close (2026-11-12) slice 2 (read-baseline,
register-burst, breakpoint and soak scenarios, plus the live Grafana dashboard)
and the busy-desktop variance set were descoped to follow-up work; see
`design.md` Outcome.

## 1. Slice 1 (first slice): smoke and auth mix against the capped stack, with seeding, budgets and baselines

This slice delivers these scenarios from `specs/performance/spec.md`:
- **Named load scenarios run with one command**: run a scenario by name,
  unknown scenario name, stack not running, generator cannot keep up (for
  `smoke` and `auth-mix`).
- **Load only targets the local stack**: remote target refused.
- **Runs pass or fail against budgets**: within budget, latency budget
  exceeded, error budget exceeded.
- **Scenarios behave like the web client**: all scenarios.
- **Test Members are created through the real sign-up flow**: all scenarios.
- **Load-tested stack is resource-capped and prod-equivalent**: all scenarios,
  including "earlier runs don't slow later ones" and "back-to-back runs wait
  for an idle stack" (both added during slice 1).
- **Results are visible live and kept for comparison**: observability stack
  down, compare with baseline, set a baseline, no baseline yet. Live view comes
  in slice 2.
- **Interrupted runs leave nothing behind**: developer interrupts a run.

### Stack

- [x] 1.1 Add `loadtest/compose.load.yaml`. It sets the backend, db and edge `cpuset`/`cpus`/`mem_limit` from `LOAD_CPUSET_*` env vars with the defaults in design Decision 3, and the backend `ARGON2_*` production values. It also adds a `k6` service (profile `loadtest`, `grafana/k6` pinned by digest, `loadtest/` mounted read-only, `loadtest/results/` writable, `cpuset` `6-11`, `mem_limit: 2g`). Verify: `docker compose -f compose.yaml -f loadtest/compose.load.yaml --profile full --profile loadtest config` renders, and `docker inspect` on the running backend shows the cpuset, the memory limit and `ARGON2_TIME_COST=3`.
- [x] 1.2 Add the recipes `just load-up` (build and start `full` with the override in the background, then wait for `/api/v1/health/ready` through the edge) and `just load-down`. Verify: `just load-up` returns once the stack is ready, and `curl localhost:${EDGE_PORT:-8080}/api/v1/health/ready` returns 200.
- [x] 1.3 Add `loadtest/results/` to `.gitignore`, and ignore `loadtest/` in prettier/eslint where k6 globals would trip them, or add a k6 globals block to `eslint.config.js`. Verify: `just lint` passes with the new files present.

### Shared k6 code

- [x] 1.4 Add `loadtest/lib/config.js`: it reads `LOAD_BASE_URL` (default `http://edge:8080`), `LOAD_USERS` (default 500) and `LOAD_ALLOW_REMOTE`, and aborts in the init context when the host isn't `edge`, `localhost` or `127.0.0.1` and the opt-in isn't set. Verify: running with `LOAD_BASE_URL=https://example.com` fails before any request (no request shows in the k6 summary), and passes with `LOAD_ALLOW_REMOTE=1` set to the point of the readiness check.
- [x] 1.5 Add `loadtest/lib/client.js`. It handles per-VU state (cookie jar, CSRF token from the login and refresh bodies, a device fingerprint generated once per VU), the `name` tag `"<METHOD> <route template>"` on every request, and per-request expected statuses. Verify: the smoke scenario (1.10) shows each request under its route-template name, with no 403s from CSRF.
- [x] 1.6 Add `loadtest/flows/auth.js` with `signIn`, `me`, `refresh`, `signOut` and the auth-mix iteration (login → me ×5 → refresh → me ×2 → logout), mapping seeded Member `i` to VU `i mod N`. Verify: auth-mix (1.11) passes against a seeded stack, and two consecutive refreshes succeed in the smoke run.
- [x] 1.7 Add `loadtest/lib/budgets.js` with the provisional budgets from design Decision 5, plus a `thresholdsFor(scenario, routes)` helper that adds the error-rate limit, `dropped_iterations: count==0`, and only `phase:measure` selectors. Verify: lowering the login p95 budget to 1 ms makes auth-mix fail and name `POST /api/v1/auth/login` p95 as exceeded. Restore the value afterwards.
- [x] 1.8 Add `handleSummary` in `loadtest/lib/summary.js`: it prints the per-route count, error rate and p50/p95/p99, and writes `loadtest/results/<scenario>-<UTC>.json` with the metadata from design Decision 10 (commit, caps, cpusets, Argon2, `DEV_MODE`, complete/incomplete). Verify: after a smoke run, the JSON file exists and contains every metadata field.

### Seeding

- [x] 1.9 Add `loadtest/seed.js` and `just load-seed n="500"`: for each `test-load-<i>@example.test`, try to sign in. On failure, register → verify-email `123456` → complete-signup. Add `just load-reset`, which deletes `test-load-%` users through `psql` the same way as `grant-admin`. Verify: seed 500 on an empty DB (500 sign in). Reset, seed 300, then seed 500 (200 created, command succeeds). `just load-reset` removes them all.

### Scenarios and the run recipe

- [x] 1.10 Add `loadtest/scenarios/smoke.js` (1 it/s for 30 s, every load-tested route, a 0% error budget), with a `setup()` that signs in as Member `N-1` and aborts with "run `just load-seed` first" on failure. Verify: on an unseeded DB it aborts with that message and sends no other requests. Seeded, it passes.
- [x] 1.11 Add `loadtest/scenarios/auth-mix.js` (30 s warm-up tagged `phase:warmup`, then 10 it/s for 3 min tagged `phase:measure`). Verify: it passes on the capped stack, and the summary shows login p95 in the hundreds of milliseconds, which confirms production hashing cost is in effect.
- [x] 1.12 Add `just load <scenario>`. It checks the name against `loadtest/scenarios/` (listing valid names on a mismatch), checks readiness through the edge within 10 s, adds Prometheus remote-write output only if `victoriametrics` resolves, passes metadata env, runs `docker compose run --rm k6` under a trap that removes the container on interrupt, then runs `loadtest/report.mjs`, and exits non-zero if either step failed. Verify: `just load nope` lists valid names. With the stack down, `just load smoke` fails in ≤10 s. With `just obs-down`, `just load smoke` still passes and writes its summary.
- [x] 1.13 Add `loadtest/report.mjs` (baseline comparison: p95 per route against `loadtest/baselines/<scenario>.json` with the % change, or "no baseline") and `just load-baseline <scenario>` (copies the latest complete result and refuses incomplete runs). Verify: the first run prints "no baseline". After `just load-baseline auth-mix`, the next run prints the per-route deltas. Ctrl-C during auth-mix prints an incomplete summary, `docker ps` shows no k6 container, and `just load-baseline auth-mix` refuses that run.
- [x] 1.14 Force the generator to fall behind (temporarily set auth-mix to 500 it/s with a low `maxVUs`), and confirm the run fails on `dropped_iterations` and reports the dropped count. Verify: observed and noted in the PR description, and the scenario file is restored.

### Variance and baseline

- [x] 1.14b Wait in `loadtest/run.sh` until `LOAD_IDLE_SECONDS` (default 120) have passed since the last load run before a measured run (design Decision 7, slice 1 finding). Verify: two smoke runs back to back, and the second prints the wait and waits.
- [x] 1.14a Clear the test Members' refresh tokens (and `VACUUM (ANALYZE)` the table) in `loadtest/run.sh` before every scenario run, so runs start from the same data (slice 1 finding, design Decision 7). Verify: the run prints the number of cleared tokens, and refresh/logout p95 no longer climb across back-to-back runs (checked in 1.15).

- [x] 1.15 Measure run-to-run variance and record it in `design.md` Hypothesis. **Quiet set: done** (2026-11-12, with the layout fix, token clearing and idle check that came out of it). **Busy set: for the developer.** With the load stack up and seeded, use the machine normally (browser, editor, builds you'd run anyway) and run `for i in 1 2 3 4 5; do just load auth-mix; done` (about 28 min with the idle waits). Then run `python3` over the last five `loadtest/results/auth-mix-*.json` (p95 mean and CV per route, as in the quiet table) and add a "busy desktop" row group to Hypothesis, saying whether the stored baseline still holds. Verify: Hypothesis has both sets and a conclusion. **Closed with the quiet set only:** the busy set was descoped at close (design Outcome), so baselines are only known to hold on a quiet desktop.
- [x] 1.16 Replace the provisional budgets in `budgets.js` with max(1.5 × worst p95 across the fresh-data variance runs, 25 ms) for p95 and max(1.5 × worst p99, 50 ms) for p99, rounded up (design Decision 5). Set baselines for `smoke` and `auth-mix` with `just load-baseline`, and record backend findings in `design.md` for follow-up. Verify: `loadtest/baselines/smoke.json` and `auth-mix.json` are committed, and both scenarios pass against them.
- [x] 1.17 Add a "Load tests" section to `README.md`: prerequisites, `load-up` / `load-seed` / `load` / `load-baseline` / `load-reset` / `load-down`, the cpuset env vars, the `DEV_MODE` differences, and that results are relative to this machine. Verify: following the section from a fresh clone (after `just setup`) runs smoke to a pass.
- [x] 1.18 `just ci` green, and slice 1 merged to `main`.

## 2. Feature done

- [x] 2.1 Every scenario in `specs/performance/spec.md` has been checked locally. Slice 2 was descoped at close and the spec trimmed to slice 1 (design Outcome).
- [x] 2.2 README and `backend/ARCHITECTURE.md` (if it lists dev tooling) reflect the final recipes and scenarios. README's "Load tests" section is from 1.17; `ARCHITECTURE.md` lists no dev tooling.
- [x] 2.3 Archive the change with `openspec archive ot-004-load-tests`, which creates `openspec/specs/performance/spec.md`.
