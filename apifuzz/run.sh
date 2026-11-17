#!/usr/bin/env bash
# Fuzz the HTTP API against its committed description (OT-011). Called by
# `just fuzz-api`.
#
#   apifuzz/run.sh [seed] [examples]
#
# Creates and migrates a throwaway database, starts the backend on it in dev
# mode, fuzzes every described operation in three passes (Visitor, signed in,
# Member on admin routes), then stops the backend and drops the database,
# whatever happens. Exits non-zero if any pass fails or if the backend or its
# database can't be started; it never reports success without sending requests.
#
# Environment:
#   FUZZ_API_PORT        backend port (default 4399)
#   FUZZ_DB_ADMIN_URL    Postgres URL with CREATEDB, used to name the server
#                        (default: the local dev database's server)
#   FUZZ_PASSES          passes to run (default "visitor signed-in member-on-admin")
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD

seed=${1:-${FUZZ_SEED:-1}}
examples=${2:-${FUZZ_EXAMPLES:-25}}
port=${FUZZ_API_PORT:-4399}
passes=${FUZZ_PASSES:-visitor signed-in member-on-admin}
[[ $seed =~ ^[0-9]+$ ]] || { echo "fuzz: seed must be a non-negative integer, got '$seed'" >&2; exit 2; }
[[ $examples =~ ^[1-9][0-9]*$ ]] || { echo "fuzz: examples must be a positive integer, got '$examples'" >&2; exit 2; }

admin_url=${FUZZ_DB_ADMIN_URL:-${DATABASE_URL:-postgres://overkill_todo:overkill_todo@localhost:${DB_PORT:-5432}/overkill_todo}}
db_name=overkill_todo_fuzz_$$
db_url="${admin_url%/*}/$db_name"
base_url="http://127.0.0.1:$port"

die() { echo "fuzz: $*" >&2; exit 1; }

work=$(mktemp -d)
backend_pid=
db_created=0
cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if [[ -n $backend_pid ]]; then
    kill -TERM "$backend_pid" 2>/dev/null || true
    wait "$backend_pid" 2>/dev/null || true
  fi
  if ((db_created)); then
    (cd backend && sqlx database drop -y -f --database-url "$db_url" >/dev/null) ||
      echo "fuzz: couldn't drop $db_name; drop it by hand" >&2
  fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# ── Database ─────────────────────────────────────────────────────────────────────
echo "fuzz: creating database $db_name"
(cd backend && sqlx database create --database-url "$db_url") ||
  die "couldn't create $db_name on ${admin_url%/*} (is Postgres up? \`just db-up\`)"
db_created=1
(cd backend && sqlx migrate run --database-url "$db_url" >/dev/null) || die "couldn't migrate $db_name"

# ── Backend ──────────────────────────────────────────────────────────────────────
echo "fuzz: building the backend"
(cd backend && SQLX_OFFLINE=true cargo build --quiet --bin overkill-todo) || die "the backend didn't build"
binary=$root/backend/target/debug/overkill-todo

# Every setting given explicitly, started from an empty directory so the
# developer's backend/.env is never read. Dev mode: console email, no-op bot and
# MX checks, the fixed code for test-* addresses (design Decision 3). Telemetry
# export is off (no OTEL endpoint).
backend_env=(
  DEV_MODE=true
  SERVER_HOST=127.0.0.1
  SERVER_PORT="$port"
  APP_NAME=overkill-todo-fuzz
  FRONTEND_BASE_URL="$base_url"
  DATABASE_URL="$db_url"
  DATABASE_MAX_CONNECTIONS=5
  ARGON2_MEMORY_COST=4096
  ARGON2_TIME_COST=1
  ARGON2_PARALLELISM=1
  JWT_EXPIRATION_MINUTES=15
  REFRESH_TOKEN_EXPIRATION_DAYS=1
  JWT_SIGNING_KEY="$(scripts/jwt-keygen.sh fuzz)"
  CSRF_SECRET="$(openssl rand -base64 32)"
  SECRETS_ENCRYPTION_KEY="$(openssl rand -base64 32)"
  # OAuth start only builds a redirect; the callback (the real exchange) is excluded.
  GOOGLE_OAUTH_ENABLED=true
  GOOGLE_CLIENT_ID=apifuzz-client
  GOOGLE_CLIENT_SECRET=apifuzz-secret
  GOOGLE_REDIRECT_URI="$base_url/api/v1/auth/oauth/google/callback"
  MX_VALIDATION_ENABLED=false
  LOG_FORMAT=json
  RUST_LOG="${FUZZ_RUST_LOG:-warn}"
)
out=$root/apifuzz/out
rm -rf "$out" && mkdir -p "$out"
log=$out/backend.log
(cd "$work" && exec env -i PATH="$PATH" HOME="$HOME" "${backend_env[@]}" "$binary") >"$log" 2>&1 &
backend_pid=$!

echo "fuzz: waiting for the backend on $base_url"
for _ in $(seq 60); do
  kill -0 "$backend_pid" 2>/dev/null || { tail -20 "$log" >&2; die "the backend exited during startup (log: $log)"; }
  curl -fsS -o /dev/null --max-time 2 "$base_url/api/v1/health/ready" && break
  sleep 0.5
done
curl -fsS -o /dev/null --max-time 2 "$base_url/api/v1/health/ready" ||
  { tail -20 "$log" >&2; die "the backend didn't become ready within 30s (log: $log)"; }

# ── Fuzz ─────────────────────────────────────────────────────────────────────────
# FUZZ_RUN_ID names the test Members (test-fuzz-<id>-<n>@example.test). Fixed per
# pass, not per run: the database is new every run, and Schemathesis reuses
# values it saw in responses, so a varying email would vary the requests.
failed=()
results=()
for pass in $passes; do
  args=()
  case $pass in
    visitor | signed-in) ;;
    member-on-admin) args+=(--include-path-regex '^/api/v1/admin/') ;;
    *) die "unknown pass '$pass'" ;;
  esac
  echo
  echo "fuzz: ── pass: $pass (seed $seed, $examples examples per operation) ──"
  pass_log=$out/$pass.log
  # Each pass's output is kept (CI uploads apifuzz/out/), and still shown.
  if FUZZ_PASS=$pass FUZZ_BASE_URL=$base_url FUZZ_DATABASE_URL=$db_url FUZZ_RUN_ID="${pass%%-*}" \
    PYTHONPATH=$root/apifuzz \
    uv run --project apifuzz --frozen --quiet schemathesis --config-file apifuzz/schemathesis.toml \
    run backend/openapi.json --url "$base_url" --seed "$seed" --max-examples "$examples" \
    --phases examples,coverage,fuzzing "${args[@]}" 2>&1 | tee "$pass_log"; then
    results+=("$pass:clean")
  else
    failed+=("$pass")
    results+=("$pass:FAILED")
  fi
done

reproduce="just fuzz-api $seed $examples"

# GitHub job summary: the seed, each pass's result and its failures, so a red
# run (nightly especially) can be reproduced without opening the logs.
if [[ -n ${GITHUB_STEP_SUMMARY:-} ]]; then
  {
    echo "## API fuzzing"
    echo
    echo "Seed **$seed**, $examples examples per operation. Reproduce locally: \`$reproduce\`"
    echo
    echo "| Pass | Result |"
    echo "| --- | --- |"
    for r in "${results[@]}"; do echo "| ${r%%:*} | ${r#*:} |"; done
    for pass in "${failed[@]}"; do
      echo
      echo "<details><summary>Failures in the $pass pass</summary>"
      echo
      echo '```'
      # The FAILURES section: each failing operation, the check, the response
      # and a curl command that reproduces it.
      sed -n '/^=* FAILURES =*$/,/^=* \(WARNINGS\|SUMMARY\) =*$/p' "$out/$pass.log" | head -300
      echo '```'
      echo "</details>"
    done
  } >>"$GITHUB_STEP_SUMMARY"
fi

echo
if ((${#failed[@]})); then
  echo "fuzz: FAILED in pass(es): ${failed[*]}. Reproduce with: $reproduce" >&2
  echo "fuzz: logs: $out/" >&2
  exit 1
fi
echo "fuzz: all passes clean (seed $seed, $examples examples per operation)"
