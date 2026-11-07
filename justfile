set dotenv-load := false

# Run every recipe (shebang ones too) with mise's tools first on PATH, so the
# pinned Rust, node, pnpm, ... win over system installs (e.g. Arch's `rust`)
# even in shells without `mise activate`. Fails fast if mise is missing.
export PATH := `mise exec -- printenv PATH || { echo 'mise is required: https://mise.jdx.dev, then "mise trust && mise install"' >&2; exit 1; }`
export RUSTUP_TOOLCHAIN := `mise exec -- printenv RUSTUP_TOOLCHAIN`

backend := "backend"
web := "@overkill-todo/web"
# Refuse npm releases younger than 24h, even if someone bypasses pnpm-workspace.yaml.
pnpm := "env PNPM_CONFIG_MINIMUM_RELEASE_AGE=1440 pnpm"
# Host-side DB access for sqlx / tests / psql. Matches compose.yaml.
db_url := "postgres://overkill_todo:overkill_todo@localhost:" + env_var_or_default("DB_PORT", "5432") + "/overkill_todo"

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

# Run backend (auto-reload) + web dev server. Web on :4321 proxies /api to :4322.
dev: db-up
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill 0' EXIT
    (cd {{backend}} && if command -v cargo-watch >/dev/null; then cargo watch -q -x run; else cargo run; fi) &
    {{pnpm}} --filter {{web}} dev &
    wait

# Prod-like stack in Docker: real images behind a Caddy edge on :8080.
up:
    docker compose --profile full up --build

down:
    docker compose --profile full down

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
    docker volume rm -f overkill-todo_db-data
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

# Frontend component-test coverage. HTML report: apps/web/coverage/index.html.
coverage-web:
    {{pnpm}} --filter {{web}} test:coverage

audit:
    cd {{backend}} && cargo audit

deny:
    cd {{backend}} && cargo deny check

# Everything CI gates on. Green here means green CI.
ci: toolchain-check fmt-check lint lint-js test test-integration db-prepare-check test-web e2e

# ── Images ───────────────────────────────────────────────────────────────────────

docker-build:
    docker build -t overkill-todo-backend {{backend}}
    docker build -t overkill-todo-web -f apps/web/Dockerfile .
