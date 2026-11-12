#!/usr/bin/env bash
# Runs one load scenario in the k6 container (OT-004). Called by `just load`.
#
#   loadtest/run.sh <scenario>        a scenario from loadtest/scenarios/
#   loadtest/run.sh --seed <n>        seed n test Members (loadtest/seed.js)
#
# Exits non-zero if the stack isn't ready, k6's thresholds fail, or report.mjs
# fails. Ctrl-C stops the run; k6 still writes an (incomplete) summary.
set -euo pipefail
cd "$(dirname "$0")/.."

compose=(docker compose -f compose.yaml -f loadtest/compose.load.yaml --profile full --profile loadtest)
edge="http://localhost:${EDGE_PORT:-8080}"

die() { echo "load: $*" >&2; exit 1; }

valid_scenarios() { (cd loadtest/scenarios && ls -- *.js | sed 's/\.js$//' | paste -sd ' '); }

# ── What to run ──────────────────────────────────────────────────────────────────
k6_env=()
if [[ ${1:-} == --seed ]]; then
  [[ ${2:-} =~ ^[1-9][0-9]*$ ]] || die "seed count must be a positive integer, got '${2:-}'"
  scenario=seed script=seed.js
  k6_env+=(-e "LOAD_USERS=$2")
else
  scenario=${1:-}
  [[ -n $scenario && -f loadtest/scenarios/$scenario.js ]] ||
    die "unknown scenario '${scenario}'. Valid scenarios: $(valid_scenarios)"
  script=scenarios/$scenario.js
  [[ -n ${LOAD_USERS:-} ]] && k6_env+=(-e "LOAD_USERS=$LOAD_USERS")
fi
[[ -n ${LOAD_BASE_URL:-} ]] && k6_env+=(-e "LOAD_BASE_URL=$LOAD_BASE_URL")
[[ -n ${LOAD_ALLOW_REMOTE:-} ]] && k6_env+=(-e "LOAD_ALLOW_REMOTE=$LOAD_ALLOW_REMOTE")

# ── Is the capped stack up? ──────────────────────────────────────────────────────
if [[ -z ${LOAD_BASE_URL:-} ]]; then
  curl -fsS --max-time 10 -o /dev/null "$edge/api/v1/health/ready" ||
    die "the stack isn't reachable at $edge/api/v1/health/ready. Start it with \`just load-up\`."

  backend=$("${compose[@]}" ps -q backend)
  [[ -n $backend ]] || die "no backend container. Start the stack with \`just load-up\`."
  read -r cpuset nanocpus memory < <(docker inspect -f '{{.HostConfig.CpusetCpus}} {{.HostConfig.NanoCpus}} {{.HostConfig.Memory}}' "$backend")
  [[ -n $cpuset && $memory != 0 ]] ||
    die "the backend isn't resource-capped (started without the load overlay?). Run \`just load-up\`."
  benv() { docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$backend" | sed -n "s/^$1=//p"; }
  db=$("${compose[@]}" ps -q db)
  read -r db_cpuset db_memory < <(docker inspect -f '{{.HostConfig.CpusetCpus}} {{.HostConfig.Memory}}' "$db")

  k6_env+=(
    -e "LOAD_META_BACKEND_CPUSET=$cpuset"
    -e "LOAD_META_BACKEND_CPUS=$(awk "BEGIN{print $nanocpus/1e9}")"
    -e "LOAD_META_BACKEND_MEMORY_MB=$((memory / 1048576))"
    -e "LOAD_META_DB_CPUSET=$db_cpuset"
    -e "LOAD_META_DB_MEMORY_MB=$((db_memory / 1048576))"
    -e "LOAD_META_ARGON2=m=$(benv ARGON2_MEMORY_COST),t=$(benv ARGON2_TIME_COST),p=$(benv ARGON2_PARALLELISM)"
    -e "LOAD_META_DEV_MODE=$(benv DEV_MODE)"
    -e "LOAD_META_DB_MAX_CONNECTIONS=$(benv DATABASE_MAX_CONNECTIONS)"
  )
fi
# Same starting data for every run: revoked refresh tokens pile up per Member
# and slow refresh/logout down as they do (OT-004 design, slice 1 findings).
if [[ -z ${LOAD_BASE_URL:-} && $scenario != seed ]]; then
  cleared=$("${compose[@]}" exec -T db psql -U overkill_todo -d overkill_todo -qtA -v ON_ERROR_STOP=1 <<'SQL'
WITH gone AS (
  DELETE FROM refresh_tokens
  WHERE user_id IN (SELECT id FROM users WHERE email LIKE 'test-load-%@example.test')
  RETURNING 1
) SELECT count(*) FROM gone;
VACUUM (ANALYZE) refresh_tokens;
SQL
  ) || die "couldn't clear the test Members' refresh tokens"
  echo "load: cleared $cleared test-Member refresh tokens"
fi

commit=$(git rev-parse --short HEAD)
git diff --quiet HEAD -- backend 2>/dev/null || commit+="-dirty"
k6_env+=(-e "LOAD_META_COMMIT=$commit" -e "LOAD_META_K6_CPUSET=${LOAD_CPUSET_K6:-3,4,9,10}")

# ── Live metrics, only when the observability stack is up ────────────────────────
out=()
stamp=$(date -u +%Y%m%dT%H%M%SZ)
if [[ -n $("${compose[@]}" --profile observability ps -q --status running victoriametrics 2>/dev/null) ]]; then
  out=(-o experimental-prometheus-rw)
  k6_env+=(
    -e K6_PROMETHEUS_RW_SERVER_URL=http://victoriametrics:8428/api/v1/write
    -e "K6_PROMETHEUS_RW_TREND_STATS=p(95),p(99),avg"
    --tag "testid=$scenario-$stamp"
  )
  echo "load: streaming metrics to VictoriaMetrics (testid=$scenario-$stamp)"
else
  echo "load: observability stack not running; no live metrics (the run is unaffected)"
fi

# ── Start clean ──────────────────────────────────────────────────────────────────
# Sustained load drives the write path into a slower state that an idle period
# clears (OT-004 design, slice 1). Measured runs start only after
# LOAD_IDLE_SECONDS without load from these scripts, so every run starts clean.
mkdir -p loadtest/results
last_end_file=loadtest/results/.last-load-end
idle=${LOAD_IDLE_SECONDS:-120}
if [[ $scenario != seed && -f $last_end_file ]] && ((idle > 0)); then
  wait_s=$(($(cat "$last_end_file") + idle - $(date +%s)))
  if ((wait_s > 0)); then
    echo "load: waiting ${wait_s}s so the stack has been idle for ${idle}s since the last run (LOAD_IDLE_SECONDS=0 skips)"
    sleep "$wait_s"
  fi
fi

# ── Run ──────────────────────────────────────────────────────────────────────────
result=$scenario-$stamp.json
name="${COMPOSE_PROJECT_NAME:-overkill-todo}-k6-$stamp"
cleanup() {
  docker rm -f "$name" >/dev/null 2>&1 || true
  date +%s >"$last_end_file"
}
trap cleanup EXIT

# Compose runs in its own session so Ctrl-C doesn't reach it (it would kill
# k6 outright). Instead the trap asks k6 itself to stop: k6 then ends the run
# and still writes its summary, marked incomplete. A second Ctrl-C kills it.
interrupted=0
on_int() {
  if ((interrupted++ == 0)); then
    echo >&2
    echo "load: interrupted; stopping k6 and writing its summary (Ctrl-C again to kill)..." >&2
    docker kill --signal INT "$name" >/dev/null 2>&1 || true
  else
    docker kill "$name" >/dev/null 2>&1 || true
  fi
}
trap on_int INT TERM

LOAD_UID=$(id -u) LOAD_GID=$(id -g) setsid "${compose[@]}" run --rm --no-deps -T --name "$name" \
  k6 run --quiet "${out[@]}" "${k6_env[@]}" -e "LOAD_RESULT_FILE=/results/$result" "$script" &
run_pid=$!
# `wait` returns early when a trapped signal arrives; keep waiting for k6.
k6_status=0
while :; do
  wait "$run_pid" && k6_status=0 || k6_status=$?
  kill -0 "$run_pid" 2>/dev/null || break
done

[[ $scenario == seed ]] && exit "$k6_status"

report_status=0
if [[ -f loadtest/results/$result ]]; then
  node loadtest/report.mjs "loadtest/results/$result" || report_status=$?
fi
case $k6_status in
  0) ;;
  99) echo "load: FAILED, limits exceeded (see above)" >&2 ;;
  *) echo "load: k6 exited with status $k6_status" >&2 ;;
esac
((k6_status == 0 && report_status == 0))
