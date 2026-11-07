set dotenv-load := false

backend := "backend"
web := "@app/web"
# Refuse npm releases younger than 24h, even if someone bypasses pnpm-workspace.yaml.
pnpm := "env PNPM_CONFIG_MINIMUM_RELEASE_AGE=1440 pnpm"
# Host-side DB access for sqlx / tests / psql. Matches compose.yaml.
db_url := "postgres://app:app@localhost:" + env_var_or_default("DB_PORT", "5432") + "/app"

default:
    @just --list --unsorted

# ── Setup ────────────────────────────────────────────────────────────────────────

# One-time: tools, deps, browsers, backend/.env, database.
setup:
    command -v mise >/dev/null && mise install || echo "mise not found; install node/pnpm/just/sqlx-cli yourself"
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
    docker volume rm -f app_db-data
    just db-up db-migrate

db-psql:
    docker compose exec db psql -U app -d app

# Promote a local user to super_admin (bootstrap the first admin).
grant-admin email:
    #!/usr/bin/env bash
    set -euo pipefail
    # Bound as a psql variable and quoted by psql (:'em'), never interpolated into SQL.
    out=$(printf '%s\n' "UPDATE users SET role = 'super_admin' WHERE email = :'em' RETURNING email, role;" \
      | docker compose exec -T db psql -U app -d app -v ON_ERROR_STOP=1 -v em={{ quote(email) }})
    echo "$out"
    if grep -q '^UPDATE 0$' <<<"$out"; then echo "no user matched" >&2; exit 1; fi

# ── Checks ───────────────────────────────────────────────────────────────────────

fmt:
    cd {{backend}} && cargo fmt
    {{pnpm}} format

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

audit:
    cd {{backend}} && cargo audit

deny:
    cd {{backend}} && cargo deny check

# Everything CI gates on. Green here means green CI.
ci: fmt-check lint lint-js test test-integration db-prepare-check test-web e2e

# ── Images ───────────────────────────────────────────────────────────────────────

docker-build:
    docker build -t app-backend {{backend}}
    docker build -t app-web -f apps/web/Dockerfile .
