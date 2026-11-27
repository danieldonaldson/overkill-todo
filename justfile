# Only .worktree.env (per-worktree ports + Compose project, from `just wt-new`),
# never a stray .env. Absent in the main checkout, which keeps the defaults.
set dotenv-load := true
set dotenv-path := ".worktree.env"

# Run every recipe (shebang ones too) with mise's tools first on PATH, so the
# pinned Rust, node, pnpm, ... win over system installs (e.g. Arch's `rust`)
# even in shells without `mise activate`. Fails fast if mise is missing.
export PATH := `mise exec -- printenv PATH || { echo 'mise is required: https://mise.jdx.dev, then "mise trust && mise install"' >&2; exit 1; }`
export RUSTUP_TOOLCHAIN := `mise exec -- printenv RUSTUP_TOOLCHAIN`
# sccache shares compiled dependencies between worktrees (each still has its own
# target/). Empty means no wrapper: when sccache isn't installed yet, in CI, or
# when you've set RUSTC_WRAPPER yourself.
export RUSTC_WRAPPER := env("RUSTC_WRAPPER", if env("CI", "") != "" { "" } else { `mise exec -- sh -c 'command -v sccache || true'` })

backend := "backend"
web := "@overkill-todo/web"
# Refuse npm releases younger than 24h, even if someone bypasses pnpm-workspace.yaml.
pnpm := "env PNPM_CONFIG_MINIMUM_RELEASE_AGE=1440 pnpm"
# Rule unit-test runner; same release as the vmalert image in compose.yaml.
vmalert_tool := "victoriametrics/vmalert-tool:v1.156.0@sha256:5db1f962282311fe37212711fce72d55983a49a627c632feb38b318d949920fa"
# Host-side DB access for sqlx / tests / psql. Matches compose.yaml.
db_url := "postgres://overkill_todo:overkill_todo@localhost:" + env("DB_PORT", "5432") + "/overkill_todo"
# Compose project: "overkill-todo" in the main checkout, per-worktree otherwise.
project := env("COMPOSE_PROJECT_NAME", "overkill-todo")
load_compose := "docker compose -f compose.yaml -f loadtest/compose.load.yaml --profile full"

# Every recipe has a [group]; `just` lists them by group, in this file's order.
[private]
default:
    @just --list --unsorted

# ── Setup ────────────────────────────────────────────────────────────────────────

# One-time: tools, deps, browsers, backend/.env, database.
[group('setup')]
setup:
    mise install
    cargo kani setup   # Kani's own toolchain for `just verify`; a no-op once done
    {{pnpm}} install
    {{pnpm}} --filter {{web}} exec playwright install chromium
    ./scripts/gen-dev-env.sh
    just db-up

# The Rust target/, web build, coverage, test reports, model-checker and fuzzer
# output all come back on the next build or run. Keeps env files, bench baselines
# (.bench/), proptest seeds, load-test results and Docker volumes. DEPS=1 also
# deletes node_modules and the fuzzer's venv (`just setup` restores them).
# Free disk: delete build and test output (DEPS=1 also deletes installed deps).
[group('setup')]
clean:
    #!/usr/bin/env bash
    set -euo pipefail
    paths=(
      {{backend}}/target {{backend}}/openapi.json.tmp
      apps/web/dist apps/web/.astro apps/web/coverage apps/web/test-results
      apps/web/playwright-report apps/web/.vitest-attachments
      apps/web/src/components/dashboard/__screenshots__
      _apalache-out apifuzz/out .hypothesis .pytest_cache apifuzz/.pytest_cache
    )
    if [[ ${DEPS:-} == 1 ]]; then
      paths+=(node_modules apps/web/node_modules packages/*/node_modules apifuzz/.venv)
    fi
    existing=()
    for p in "${paths[@]}"; do [[ -e $p ]] && existing+=("$p"); done
    if (( ${#existing[@]} == 0 )); then echo "nothing to clean"; exit 0; fi
    du -shc "${existing[@]}" | tail -1 | sed 's/total$/to free/'
    rm -rf -- "${existing[@]}"
    find . \( -path ./node_modules -o -path ./{{backend}}/target -o -path ./apifuzz/.venv \) -prune -o \
      -type d -name __pycache__ -prune -exec rm -rf {} +
    echo "cleaned"

# ── Local dev ────────────────────────────────────────────────────────────────────

# Backend (auto-reload) + web dev server. Web :4321 proxies /api to :4322 (WEB_PORT/API_PORT in a worktree).
# Each half runs in its own process group (setsid), and the trap kills both
# groups, so Ctrl-C leaves nothing behind holding the ports. Astro runs directly:
# `pnpm --filter … dev` moves it into a group of its own, out of reach of the trap.
[group('dev')]
[doc('Backend (auto-reload) + web dev server. Web :4321 proxies /api to :4322 (WEB_PORT/API_PORT in a worktree).')]
dev: db-up
    #!/usr/bin/env bash
    set -euo pipefail
    pids=()
    cleanup() {
        trap - EXIT INT TERM
        for pid in "${pids[@]}"; do kill -TERM -- "-$pid" 2>/dev/null || true; done
        wait 2>/dev/null || true
    }
    trap cleanup EXIT
    trap 'exit 0' INT TERM  # Ctrl-C is how you stop it, not a failure
    setsid bash -c "cd {{backend}} && if command -v cargo-watch >/dev/null; then exec cargo watch -q -x 'run -p server'; else exec cargo run -p server; fi" &
    pids+=($!)
    setsid bash -c 'cd apps/web && exec node_modules/.bin/astro dev' &
    pids+=($!)
    wait -n

# Prod-like stack in Docker: real images behind a Caddy edge on :8080 (EDGE_PORT).
[group('dev')]
up:
    docker compose --profile full up --build

# Stop the `just up` stack.
[group('dev')]
down:
    docker compose --profile full down

# ── Worktrees ────────────────────────────────────────────────────────────────────

# Parallel checkout at ../overkill-todo-<branch>: own database, ports, backend/.env.
[group('worktrees')]
wt-new branch base="HEAD":
    ./scripts/worktree-new.sh {{ quote(branch) }} {{ quote(base) }}

# Remove a wt-new checkout and its database (keeps the branch; FORCE=1 if dirty).
[group('worktrees')]
wt-rm branch:
    ./scripts/worktree-rm.sh {{ quote(branch) }}

# Every checkout with its ports.
[group('worktrees')]
wt-list:
    #!/usr/bin/env bash
    set -euo pipefail
    git worktree list --porcelain | sed -n 's/^worktree //p' | while IFS= read -r p; do
      if [[ -f $p/.worktree.env ]]; then
        ports=$(sed -n 's/^\(WEB\|API\|DB\)_PORT=/\1 /p' "$p/.worktree.env" | paste -sd ' ')
      else
        ports="WEB 4321 API 4322 DB 5432"
      fi
      printf '%-50s %s\n' "$p" "$ports"
    done

# ── Database ─────────────────────────────────────────────────────────────────────

# Start the local Postgres and wait until it's healthy.
[group('database')]
db-up:
    docker compose up -d --wait db

# Apply pending migrations to the local database.
[group('database')]
db-migrate:
    cd {{backend}} && DATABASE_URL={{db_url}} cargo sqlx migrate run

# Regenerate the committed offline query cache. Run after changing any query!.
[group('database')]
db-prepare: db-migrate
    cd {{backend}} && DATABASE_URL={{db_url}} cargo sqlx prepare --workspace -- --all-targets

# Fail if the committed offline query cache is stale.
[group('database')]
db-prepare-check:
    cd {{backend}} && DATABASE_URL={{db_url}} cargo sqlx prepare --workspace --check -- --all-targets || { echo "SQLx offline cache is stale. Run 'just db-prepare' and commit backend/.sqlx/" >&2; exit 1; }

# Drop the local database volume and recreate it. Local only.
[group('database')]
db-reset:
    docker compose rm -sfv db
    docker volume rm -f {{project}}_db-data
    just db-up db-migrate

# psql shell on the local database.
[group('database')]
db-psql:
    docker compose exec db psql -U overkill_todo -d overkill_todo

# Promote a local user to super_admin (bootstrap the first admin).
[group('database')]
grant-admin email:
    #!/usr/bin/env bash
    set -euo pipefail
    # Bound as a psql variable and quoted by psql (:'em'), never interpolated into SQL.
    # One statement: the role change and its audit event (OT-008, actor
    # "operator") commit together, and nothing is written if the role was
    # already super_admin.
    sql="WITH target AS (
           SELECT id, email, role FROM users WHERE email = :'em' FOR UPDATE
         ), changed AS (
           UPDATE users u
           SET role = 'super_admin', updated_at = NOW(), version = u.version + 1
           FROM target t
           WHERE u.id = t.id AND t.role <> 'super_admin'
           RETURNING u.id, t.role AS old_role
         ), audited AS (
           INSERT INTO audit_events (kind, subject_id, actor_kind, details)
           SELECT 'role_changed', id, 'operator',
                  jsonb_build_object('from', old_role::text, 'to', 'super_admin')
           FROM changed
         )
         SELECT t.email, 'super_admin' AS role, EXISTS (SELECT 1 FROM changed) AS changed
         FROM target t;"
    out=$(printf '%s\n' "$sql" \
      | docker compose exec -T db psql -U overkill_todo -d overkill_todo -v ON_ERROR_STOP=1 -v em={{ quote(email) }})
    echo "$out"
    if grep -q '^(0 rows)$' <<<"$out"; then echo "no user matched" >&2; exit 1; fi

# ── API contract ─────────────────────────────────────────────────────────────────
# backend/openapi.json is generated from the route tree; the web types are generated
# from it. Run `just api-types` after changing a DTO, handler annotation or route,
# and commit both files. Never edit either by hand.

# Write the OpenAPI description to backend/openapi.json. No database needed.
[group('api')]
api-spec:
    cd {{backend}} && SQLX_OFFLINE=true cargo run --quiet -p api --example export_openapi > openapi.json.tmp && mv openapi.json.tmp openapi.json

# Fail if backend/openapi.json is stale.
[group('api')]
api-spec-check: api-spec
    git diff --exit-code -- {{backend}}/openapi.json || { echo "backend/openapi.json is stale. Run: just api-types" >&2; exit 1; }

# Generate the web API types from backend/openapi.json.
[group('api')]
api-ts:
    {{pnpm}} --filter {{web}} exec openapi-typescript ../../backend/openapi.json -o src/lib/api/schema.d.ts

# Fail if the web API types are stale.
[group('api')]
api-ts-check: api-ts
    git diff --exit-code -- apps/web/src/lib/api/schema.d.ts || { echo "apps/web/src/lib/api/schema.d.ts is stale. Run: just api-types" >&2; exit 1; }

# Regenerate the OpenAPI description and the web API types.
[group('api')]
api-types: api-spec api-ts

# Fail if either generated API file is stale.
[group('api')]
api-types-check: api-spec-check api-ts-check

# ── Format & lint ────────────────────────────────────────────────────────────────

# Format the backend (cargo fmt) and the whole repo (prettier).
[group('format & lint')]
fmt:
    cd {{backend}} && cargo fmt --all
    {{pnpm}} format

# Fail if the backend isn't cargo-fmt clean.
[group('format & lint')]
fmt-check:
    cd {{backend}} && cargo fmt --all --check

# Clippy, pedantic + nursery, warnings denied.
[group('format & lint')]
lint:
    cd {{backend}} && SQLX_OFFLINE=true cargo clippy --workspace --all-targets -- -D warnings

# Release-feature clippy (matches the Dockerfile's --no-default-features build).
[group('format & lint')]
lint-release:
    cd {{backend}} && SQLX_OFFLINE=true cargo clippy --workspace --lib --bins --no-default-features -- -D warnings

# Frontend lint.
[group('format & lint')]
lint-js:
    {{pnpm}} lint

# Fail if the repo isn't Prettier clean (.prettierignore lists the exemptions).
[group('format & lint')]
fmt-check-js:
    {{pnpm}} format:check

# .mise.toml and backend/rust-toolchain.toml must pin the same Rust.
[group('format & lint')]
toolchain-check:
    ./scripts/check-rust-pin.sh

# ── Tests ────────────────────────────────────────────────────────────────────────

# Backend unit tests: reach nothing (no DB).
[group('test')]
test:
    cd {{backend}} && SQLX_OFFLINE=true cargo nextest run --workspace --lib

# Backend integration tests: each #[sqlx::test] gets a fresh database.
[group('test')]
test-integration: db-up db-migrate
    cd {{backend}} && DATABASE_URL={{db_url}} cargo nextest run --workspace --tests

# Frontend type check (astro check) and component tests (Vitest).
[group('test')]
test-web:
    {{pnpm}} --filter {{web}} exec astro check
    {{pnpm}} --filter {{web}} exec vitest run
    node scripts/decode-stack.test.mjs

# Reads the stack from a `client_error` log line on stdin or from a file. `dev`
# uses apps/web/dist (build first); a commit SHA fetches that release's maps (`gh`).
# Decode a minified browser error stack to source files and lines (OT-006).
[group('test')]
[no-cd]
decode-stack release file="":
    node {{ quote(justfile_directory() / "scripts/decode-stack.mjs") }} {{ quote(release) }} {{ if file == "" { "" } else { quote(file) } }}

# Load tooling checks that need no stack (OT-013): unknown scenario, stack down,
# dropped iterations in the summary. Running a scenario needs `just load-up`.
[group('test')]
test-loadtest:
    node --test loadtest/tooling.test.mjs

# Browser e2e against the built site with the API mocked.
[group('test')]
e2e:
    PUBLIC_TELEMETRY_TEST_HOOKS=true {{pnpm}} --filter {{web}} build
    {{pnpm}} --filter {{web}} test:e2e

# Real-stack e2e (signup against the live backend). Needs `just dev` running.
[group('test')]
e2e-integration:
    {{pnpm}} --filter {{web}} test:e2e:integration

# Read-only checks against a deployed environment, e.g. just smoke https://app.example.com
# (for https, first checks the edge negotiates post-quantum key exchange).
[group('test')]
[doc('Read-only checks against a deployed env, e.g. `just smoke https://app.example.com` (https: checks PQ key exchange first).')]
smoke url:
    ./scripts/smoke-tls.sh {{url}}
    SMOKE_BASE_URL={{url}} {{pnpm}} --filter {{web}} test:e2e:smoke

# ── API fuzzing (OT-011) ──────────────────────────────────────────────────────────
# Schemathesis against the committed backend/openapi.json, on a throwaway database
# and a dev-mode backend on FUZZ_API_PORT (4399): Visitor, signed-in and
# Member-on-admin passes. Same seed, same requests. See apifuzz/run.sh.

# Fuzz the API against its description (part of `just ci`); a failure prints its seed.
[group('test')]
fuzz-api seed="1" examples="25": db-up
    ./apifuzz/run.sh {{ quote(seed) }} {{ quote(examples) }}

# Unit tests for the fuzz checks (deny list, error-body rules).
[group('test')]
fuzz-api-test:
    uv run --project apifuzz --frozen pytest -q apifuzz/tests

# ── Coverage & benchmarks ────────────────────────────────────────────────────────
# Regression workflow: `just bench --save-baseline main` on main, then on your
# branch `just bench --baseline main` (criterion prints the change per bench).
# Same machine, same power state; HTML reports land in backend/target/criterion.

# Backend coverage over unit + integration tests. HTML report: backend/target/llvm-cov/html.
[group('coverage & bench')]
coverage: db-up db-migrate
    cd {{backend}} && DATABASE_URL={{db_url}} cargo llvm-cov nextest --workspace --tests --html
    cd {{backend}} && cargo llvm-cov report --summary-only

# Frontend component-test coverage. HTML report: apps/web/coverage/index.html.
[group('coverage & bench')]
coverage-web:
    {{pnpm}} --filter {{web}} test:coverage

# Backend benches that need no DB (crypto, domain, middleware). Args go to criterion.
[group('coverage & bench')]
bench *args:
    cd {{backend}} && SQLX_OFFLINE=true cargo bench -p server --bench crypto --bench domain --bench middleware -- {{args}}

# Full-router benches against Postgres. Recreates an `overkill_todo_bench` database each run.
[group('coverage & bench')]
bench-db *args: db-up
    cd {{backend}} && DATABASE_URL={{db_url}} cargo bench -p server --bench http -- {{args}}

# Frontend benches (Vitest bench in Chromium). Compare against a base ref: `just bench-compare --web`.
[group('coverage & bench')]
bench-web *args:
    {{pnpm}} --filter {{web}} exec vitest bench --run {{args}}

# What CI runs on PRs (.github/workflows/bench.yml), here.
# Benchmark a base ref (default main) vs the working tree. Flags: --backend, --web.
[group('coverage & bench')]
bench-compare *args:
    ./scripts/bench-compare.sh {{args}}

# ── Dependencies ─────────────────────────────────────────────────────────────────

# RustSec advisory scan (the advisory part of `just deny`).
[group('dependencies')]
audit:
    cd {{backend}} && cargo deny check advisories

# cargo-deny: advisories, licenses, bans and sources.
[group('dependencies')]
deny:
    cd {{backend}} && cargo deny check

# Unused dependencies. False positives go in [package.metadata.cargo-machete].
[group('dependencies')]
machete:
    cd {{backend}} && cargo machete

# ── Formal verification (OT-010) ─────────────────────────────────────────────────

# Kani proofs of the auth policy (OT-010) and todo ordering (OT-013): every harness, every input within
# its stated bounds. Needs the pinned Kani from .mise.toml, then `cargo kani setup` once.
# `-j` checks harnesses in parallel (Kani requires terse output for it), so this
# takes about as long as the slowest one.
[group('formal')]
[doc('Kani proofs of the domain rules: every harness, every input within its bounds.')]
verify:
    @cargo kani --version >/dev/null 2>&1 || { echo "Kani isn't installed: run 'mise install', then 'cargo kani setup' once" >&2; exit 1; }
    cd {{backend}} && SQLX_OFFLINE=true cargo kani --workspace -j --output-format=terse

# Quint concurrency models (OT-010): bounded checks of every invariant, the old
# code's counterexamples, then traces replayed against Postgres. Quint + Java from .mise.toml.
[group('formal')]
[doc('Quint concurrency models: invariants, old counterexamples, traces replayed against Postgres.')]
models: db-up db-migrate
    DATABASE_URL={{db_url}} ./scripts/models.sh

# Tamarin protocol model (OT-010): every sign-in lemma proven against an active
# network attacker, and each broken variant still attacked. Tamarin + Maude from .mise.toml.
[group('formal')]
[doc('Tamarin protocol model: sign-in lemmas proven against an active network attacker.')]
protocols:
    ./scripts/protocols.sh

# Formal coverage (OT-013 D13): every list scenario names a proof, model, replay or
# test (with why no proof applies) that exists and runs; docs/formal-coverage.yaml.
# Runs the script's fixture tests first. `--final` refuses pending entries (task 9.1).
[group('formal')]
[doc('Every list scenario names a check that exists and runs (docs/formal-coverage.yaml).')]
formal-coverage *args:
    ./scripts/formal-coverage.test.sh >/dev/null
    ./scripts/formal-coverage.sh {{args}}

# ── Kubernetes ───────────────────────────────────────────────────────────────────
# A local Talos VM that ArgoCD fills from Git. See deploy/k8s/README.md.

# Create the local cluster (asks for sudo: the VM, its network, one firewall opening).
[group('kubernetes')]
k8s-up:
    ./scripts/k8s.sh up

# Destroy the local cluster, including a half-created one.
[group('kubernetes')]
k8s-down:
    ./scripts/k8s.sh down

# What is out of sync, failing or stuck in the local cluster.
[group('kubernetes')]
k8s-status:
    ./scripts/k8s.sh status

# First run only: start the cluster's database empty. Refused if any backup exists.
[group('kubernetes')]
k8s-db-init:
    ./scripts/k8s.sh db-init

# Render every Kubernetes manifest and check it against its schema. Network, no keys.
[group('kubernetes')]
k8s-check:
    ./scripts/k8s-check.sh

# ── Secrets & security ───────────────────────────────────────────────────────────
# SOPS + age. Policy: deploy/secrets/access.yaml. See deploy/secrets/README.md.

# Policy valid, .sops.yaml generated from it, every secret encrypted to exactly its readers. No keys needed.
[group('secrets & security')]
secrets-check:
    ./scripts/secrets.sh check

# After editing access.yaml: regenerate .sops.yaml and re-key the secrets you can read.
[group('secrets & security')]
secrets-sync:
    ./scripts/secrets.sh sync

# Encrypt stdin into a new secret, e.g. `openssl rand -base64 32 | just secrets-new deploy/secrets/prod/x.sops`.
[group('secrets & security')]
secrets-new path:
    ./scripts/secrets.sh new {{path}}

# On a host: decrypt one environment's secrets into deploy/compose/.secrets/.
[group('secrets & security')]
secrets-decrypt env:
    ./scripts/secrets.sh decrypt {{env}}

# Print a new token key (`<kid>:<secret>`) for JWT_SIGNING_KEY / JWT_TRUSTED_KEYS; kid defaults to today.
[group('secrets & security')]
jwt-keygen kid="":
    ./scripts/jwt-keygen.sh {{kid}}

# Every property in docs/security/threat-model.md names a check (or a gap) that still exists.
[group('secrets & security')]
threat-model-check:
    ./scripts/threat-model-check.sh

# ── Observability & analytics ────────────────────────────────────────────────────

# Observability stack (collector, Tempo, VictoriaLogs, VictoriaMetrics, vmalert,
# Alertmanager on :9093, Grafana on :3000). With `just dev`, export
# OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318, and start the stack with
# BACKEND_READY_URL=http://host.docker.internal:4322/health/ready for the probe.
[group('observability')]
[doc('Start the observability stack (Grafana :3000, Alertmanager :9093). See the justfile for use with `just dev`.')]
obs-up:
    docker compose --profile observability up -d

# Stop the observability stack.
[group('observability')]
obs-down:
    docker compose --profile observability down

# Delete all stored traces, logs and metrics (Grafana and Alertmanager state is
# kept). Restarts the stores if the stack was running. Local only.
[group('observability')]
[doc('Delete all stored traces, logs and metrics (Grafana and Alertmanager state kept). Local only.')]
obs-wipe:
    #!/usr/bin/env bash
    set -euo pipefail
    # The collector mounts the store volumes read-only (disk usage), so it goes too.
    services=(otel-collector tempo victorialogs victoriametrics)
    running=$(docker compose --profile observability ps -q "${services[@]}")
    docker compose --profile observability rm -sf "${services[@]}"
    for v in tempo-data vlogs-data vmetrics-data; do docker volume rm -f "{{project}}_$v"; done
    if [[ -n $running ]]; then docker compose --profile observability up -d --wait "${services[@]}"; fi
    echo "telemetry wiped"

# Check the observability config with the pinned images (part of `just ci`):
# collector config, alert rule unit tests, and both Alertmanager configs.
[group('observability')]
[doc('Check collector config, alert rule unit tests and both Alertmanager configs with the pinned images.')]
obs-check:
    #!/usr/bin/env bash
    set -euo pipefail
    images=$(docker compose --profile observability config --images)
    obs=$PWD/deploy/observability
    docker run --rm -v "$obs/collector/config.yaml:/c.yaml:ro" "$(grep opentelemetry-collector <<<"$images")" validate --config=/c.yaml
    docker run --rm -v "$obs/alerts:/alerts:ro" {{vmalert_tool}} unittest --disableAlertgroupLabel --files=/alerts/tests/*.yml
    for f in alertmanager.yml alertmanager.prod.yml; do
      docker run --rm --entrypoint amtool -v "$obs/alertmanager:/am:ro" "$(grep alertmanager <<<"$images")" check-config "/am/$f"
    done

# Umami web analytics (OT-005) on http://localhost:3001. Reports are local-only;
# the edge exposes just /stats/script.js and /stats/api/send. See README.
[group('observability')]
[doc('Start Umami web analytics on http://localhost:3001 (reports are local-only).')]
analytics-up:
    docker compose --profile analytics up -d --wait

# Stop Umami.
[group('observability')]
analytics-down:
    docker compose --profile analytics down

# Check the 90-day retention SQL against a seeded copy of the live Umami schema.
# Needs `just analytics-up`. Re-run after upgrading Umami.
[group('observability')]
[doc('Check the 90-day Umami retention SQL against a seeded copy of the schema. Needs `just analytics-up`.')]
analytics-retention-check:
    ./scripts/check-analytics-retention.sh

# ── Load tests (OT-004) ──────────────────────────────────────────────────────────
# Same release images as `just up`, pinned to cores and capped, with production
# hashing cost (loadtest/compose.load.yaml). Results are relative to this
# machine. See README "Load tests".

# Build and start the capped stack in the background; returns once the edge says ready.
[group('load test')]
load-up:
    #!/usr/bin/env bash
    set -euo pipefail
    {{load_compose}} up -d --build --wait
    url="http://localhost:${EDGE_PORT:-8080}/api/v1/health/ready"
    for _ in $(seq 60); do curl -fsS -o /dev/null --max-time 2 "$url" && { echo "ready: $url"; exit 0; }; sleep 1; done
    echo "the stack didn't become ready at $url" >&2; exit 1

# Stop the load-test stack.
[group('load test')]
load-down:
    {{load_compose}} --profile loadtest down

# Create n verified test Members (test-load-<i>@example.test) through the real sign-up flow. Safe to repeat.
[group('load test')]
load-seed n="500":
    ./loadtest/run.sh --seed {{n}}

# Run a scenario (smoke, auth-mix, ...); fails if a limit is exceeded. `just load` lists them.
[group('load test')]
load scenario="":
    ./loadtest/run.sh {{ quote(scenario) }}

# Make the latest complete run of a scenario its baseline (commit loadtest/baselines/ afterwards).
[group('load test')]
load-baseline scenario:
    node loadtest/report.mjs --set-baseline {{ quote(scenario) }}

# Delete every seeded test Member (and, by cascade, their sessions). Local only.
[group('load test')]
load-reset:
    #!/usr/bin/env bash
    set -euo pipefail
    printf '%s\n' "DELETE FROM users WHERE email LIKE 'test-load-%@example.test';" \
      | docker compose exec -T db psql -U overkill_todo -d overkill_todo -v ON_ERROR_STOP=1

# ── Images ───────────────────────────────────────────────────────────────────────

# Build the backend and web images locally.
[group('images')]
docker-build:
    docker build -t overkill-todo-backend {{backend}}
    docker build -t overkill-todo-web -f apps/web/Dockerfile .

# ── CI ───────────────────────────────────────────────────────────────────────────

# Everything CI gates on (coverage is standalone via `just coverage`). Green here
# means green CI. The formal checks (Kani, Quint, Tamarin) are independent of each
# other and of the rest, so they run alongside `ci-checks` rather than after it;
# wall time is the slowest of the four, not the sum. Their output interleaves; to
# read one stage's output, rerun it alone (`just models`).
[group('ci')]
[parallel]
[doc('Everything CI gates on; green here means green CI. Formal checks run alongside ci-checks.')]
ci: ci-checks verify models protocols

# The rest of `ci`, in order: static checks first, then tests.
[group('ci')]
ci-checks: toolchain-check fmt-check fmt-check-js lint lint-release deny machete lint-js obs-check secrets-check k8s-check threat-model-check formal-coverage test test-integration db-prepare-check api-types-check fuzz-api-test fuzz-api test-web test-loadtest e2e
