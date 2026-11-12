# OT-004: Load tests

## Why

As the developer of overkill-todo, I want to put repeatable load on the service
and get a pass/fail verdict, so that I know where it breaks and catch
performance regressions before Members feel them.

Nothing today tells us how the backend behaves under concurrent use. Sign-in
costs one deliberately expensive password hash per request, every request is
authenticated and CSRF-checked, and the database pool is small by default. We
don't know the throughput at which sign-in latency climbs, whether the pool runs
out first, or whether anything slowly leaks. OT-001 gave us the dashboards to
see these effects. What's missing is a reliable way to cause them. The cheapest
time to set a baseline is now, before todo features add new hot paths.

## What Changes

- A set of named load scenarios that the developer runs with one command on
  their own machine against the prod-like local stack (release images behind the
  edge):
  - **smoke**: one simulated Member for 30 s, touching every load-tested route.
  - **read baseline**: a fixed request rate on the session check and readiness
    probe.
  - **auth mix**: realistic sign-in, session check, token refresh and sign-out.
  - **register burst**: a spike of new sign-ups.
  - **breakpoint**: a rising request rate that stops when latency or errors pass
    their limits, reporting the rate it reached.
  - **soak**: moderate load for 30–60 minutes.
- Each scenario has latency (p95, p99) and error-rate limits. The run fails if
  any are exceeded.
- Load-tested services run with fixed CPU and memory limits and
  production-equivalent password-hashing cost. This keeps results comparable
  from run to run and avoids flattering sign-in.
- Results appear live in the existing operations dashboard, next to the
  backend's own request metrics. A summary with the scenario's numbers is saved
  so runs can be compared.
- The test users are created through the real sign-up flow before a run, and
  the scenarios follow the same cookie and CSRF rules as the web client.
- Scenarios are organised so todo flows can be added once todo endpoints exist.

**Outcome (2026-11-12):** closed after slice 1, which built smoke and auth mix.
The other four scenarios and the live dashboard were descoped to follow-up
work (`design.md` Outcome).

## Capabilities

### New Capabilities

- `performance`: the latency and error budgets the service is held to under
  load, and the developer's ability to measure them repeatably.

### Modified Capabilities

None.

## Impact

- **New top-level `loadtest/`** with scenarios, shared helpers, the seed step and
  a compose override that adds resource caps.
- **`justfile`**: a `just load <scenario>` recipe and a seed recipe. Not part of
  `just ci`.
- **Observability**: VictoriaMetrics accepts load-generator metrics, and Grafana
  gets a load-test dashboard. No change to production observability.
- **Backend**: no code changes are expected. Findings from the runs (such as pool
  size or hashing concurrency) become follow-up work, not part of this change.
- **Machine**: runs compete with the developer's other work for CPU. Results are
  relative to a recorded baseline on this machine, not a capacity figure for
  production.

## Appetite

About 1.5 weeks, in two slices.

## Out of Scope

- Load from other machines or the cloud, and testing the production host.
- Browser-level (page render) load tests. The frontend is static files behind the
  edge.
- Running load tests in CI. A smoke run in CI may come later.
- OAuth flows, which depend on an external provider.
- Fixing whatever the tests find. Each finding becomes its own change or task.
- Todo flows. There are no todo endpoints yet.

## Dependencies

- OT-001's observability stack (`just obs-up`) for live results. A run still
  works and still passes or fails without it.
- The prod-like local stack (`just up`).
- No data migrations, and no external services.

## Ready

- **Riskiest unknown:** whether results on a shared developer machine are stable
  enough from run to run (with the load generator competing for the same CPU) to
  serve as a regression baseline.
- **First slice:** smoke and auth-mix scenarios against the capped stack, with
  thresholds and seeding, run repeatedly to measure run-to-run variance and set
  the first baseline.
