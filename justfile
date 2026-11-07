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
# Host-side DB access for sqlx / tests / psql. Matches compose.yaml.
db_url := "postgres://overkill_todo:overkill_todo@localhost:" + env("DB_PORT", "5432") + "/overkill_todo"
# Compose project: "overkill-todo" in the main checkout, per-worktree otherwise.
project := env("COMPOSE_PROJECT_NAME", "overkill-todo")

default:
    @just --list --unsorted

# ── Setup ────────────────────────────────────────────────────────────────────────

# One-time: tools, deps, browsers, backend/.env, database.
setup:
    mise install
    {{pnpm}} install
    {{pnpm}} --filter {{web}} exec playwright install chromium
    ./scripts/gen-dev-env.sh
    just db-up

# ── Local dev ────────────────────────────────────────────────────────────────────

# Backend (auto-reload) + web dev server. Web :4321 proxies /api to :4322 (WEB_PORT/API_PORT in a worktree).
dev: db-up
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill 0' EXIT
    (cd {{backend}} && if command -v cargo-watch >/dev/null; then cargo watch -q -x run; else cargo run; fi) &
    {{pnpm}} --filter {{web}} dev &
    wait

# Prod-like stack in Docker: real images behind a Caddy edge on :8080 (EDGE_PORT).
up:
    docker compose --profile full up --build

down:
    docker compose --profile full down

# ── Worktrees ────────────────────────────────────────────────────────────────────

# Parallel checkout at ../overkill-todo-<branch>: own database, ports, backend/.env.
wt-new branch base="HEAD":
    ./scripts/worktree-new.sh {{ quote(branch) }} {{ quote(base) }}

# Remove a wt-new checkout and its database (keeps the branch; FORCE=1 if dirty).
wt-rm branch:
    ./scripts/worktree-rm.sh {{ quote(branch) }}

# Every checkout with its ports.
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

# ── Secrets ──────────────────────────────────────────────────────────────────────
# SOPS + age. Policy: deploy/secrets/access.yaml. See deploy/secrets/README.md.

# Policy valid, .sops.yaml generated from it, every secret encrypted to exactly its readers. No keys needed.
secrets-check:
    ./scripts/secrets.sh check

# After editing access.yaml: regenerate .sops.yaml and re-key the secrets you can read.
secrets-sync:
    ./scripts/secrets.sh sync

# Encrypt stdin into a new secret, e.g. `openssl rand -base64 32 | just secrets-new deploy/secrets/prod/x.sops`.
secrets-new path:
    ./scripts/secrets.sh new {{path}}

# On a host: decrypt one environment's secrets into deploy/compose/.secrets/.
secrets-decrypt env:
    ./scripts/secrets.sh decrypt {{env}}

# ── Database ─────────────────────────────────────────────────────────────────────

db-up:
    docker compose up -d --wait db

db-migrate:
    cd {{backend}} && DATABASE_URL={{db_url}} cargo sqlx migrate run

# Regenerate the committed offline query cache. Run after changing any query!.
db-prepare: db-migrate
    cd {{backend}} && DATABASE_URL={{db_url}} cargo sqlx prepare -- --all-targets

db-prepare-check:
    cd {{backend}} && DATABASE_URL={{db_url}} cargo sqlx prepare --check -- --all-targets

# Drop the local database volume and recreate it. Local only.
db-reset:
    docker compose rm -sfv db
    docker volume rm -f {{project}}_db-data
    just db-up db-migrate

db-psql:
    docker compose exec db psql -U overkill_todo -d overkill_todo

# Promote a local user to super_admin (bootstrap the first admin).
grant-admin email:
    #!/usr/bin/env bash
    set -euo pipefail
    # Bound as a psql variable and quoted by psql (:'em'), never interpolated into SQL.
    out=$(printf '%s\n' "UPDATE users SET role = 'super_admin' WHERE email = :'em' RETURNING email, role;" \
      | docker compose exec -T db psql -U overkill_todo -d overkill_todo -v ON_ERROR_STOP=1 -v em={{ quote(email) }})
    echo "$out"
    if grep -q '^UPDATE 0$' <<<"$out"; then echo "no user matched" >&2; exit 1; fi

# ── Checks ───────────────────────────────────────────────────────────────────────

fmt:
    cd {{backend}} && cargo fmt
    {{pnpm}} format

# .mise.toml and backend/rust-toolchain.toml must pin the same Rust.
toolchain-check:
    ./scripts/check-rust-pin.sh

fmt-check:
    cd {{backend}} && cargo fmt --check

lint:
    cd {{backend}} && SQLX_OFFLINE=true cargo clippy --all-targets -- -D warnings -W clippy::pedantic -W clippy::nursery

lint-js:
    {{pnpm}} lint

# Backend unit tests: reach nothing (no DB).
test:
    cd {{backend}} && SQLX_OFFLINE=true cargo nextest run --lib

# Backend integration tests: each #[sqlx::test] gets a fresh database.
test-integration: db-up db-migrate
    cd {{backend}} && DATABASE_URL={{db_url}} cargo nextest run --tests

test-web:
    {{pnpm}} --filter {{web}} exec astro check
    {{pnpm}} --filter {{web}} exec vitest run

# Browser e2e against the built site with the API mocked.
e2e:
    {{pnpm}} --filter {{web}} build
    {{pnpm}} --filter {{web}} test:e2e

# Real-stack e2e (signup against the live backend). Needs `just dev` running.
e2e-integration:
    {{pnpm}} --filter {{web}} test:e2e:integration

# Read-only checks against a deployed environment, e.g. just smoke https://app.example.com
smoke url:
    SMOKE_BASE_URL={{url}} {{pnpm}} --filter {{web}} test:e2e:smoke

# Backend coverage over unit + integration tests. HTML report: backend/target/llvm-cov/html.
coverage: db-up db-migrate
    cd {{backend}} && DATABASE_URL={{db_url}} cargo llvm-cov nextest --tests --html
    cd {{backend}} && cargo llvm-cov report --summary-only

# ── Benchmarks ───────────────────────────────────────────────────────────────────
# Regression workflow: `just bench --save-baseline main` on main, then on your
# branch `just bench --baseline main` (criterion prints the change per bench).
# Same machine, same power state; HTML reports land in backend/target/criterion.

# Backend benches that need no DB (crypto, domain, middleware). Args go to criterion.
bench *args:
    cd {{backend}} && SQLX_OFFLINE=true cargo bench --bench crypto --bench domain --bench middleware -- {{args}}

# Full-router benches against Postgres. Recreates an `overkill_todo_bench` database each run.
bench-db *args: db-up
    cd {{backend}} && DATABASE_URL={{db_url}} cargo bench --bench http -- {{args}}

# Frontend benches (Vitest bench in Chromium). Compare against a base ref: `just bench-compare --web`.
bench-web *args:
    {{pnpm}} --filter {{web}} exec vitest bench --run {{args}}

# What CI runs on PRs (.github/workflows/bench.yml), here.
# Benchmark a base ref (default main) vs the working tree. Flags: --backend, --web.
bench-compare *args:
    ./scripts/bench-compare.sh {{args}}

# Frontend component-test coverage. HTML report: apps/web/coverage/index.html.
coverage-web:
    {{pnpm}} --filter {{web}} test:coverage

# RustSec advisory scan (the advisory part of `just deny`).
audit:
    cd {{backend}} && cargo deny check advisories

deny:
    cd {{backend}} && cargo deny check

# Unused dependencies. False positives go in [package.metadata.cargo-machete].
machete:
    cd {{backend}} && cargo machete

# Everything CI gates on. Green here means green CI.
ci: toolchain-check fmt-check lint machete lint-js secrets-check test test-integration db-prepare-check test-web e2e

# ── Images ───────────────────────────────────────────────────────────────────────

docker-build:
    docker build -t overkill-todo-backend {{backend}}
    docker build -t overkill-todo-web -f apps/web/Dockerfile .
