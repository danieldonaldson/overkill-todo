#!/usr/bin/env bash
# Runs deploy/analytics/retention.sql once a day against Umami's database and
# reports the last successful run to VictoriaMetrics (OT-005, design D8).
#
# The metric is re-pushed every hour, not only after a run: VictoriaMetrics
# treats a series without fresh samples as gone, and the alert must see an old
# timestamp, not nothing. On start the last-success time is the start time, so
# a job that never succeeds still alerts (like the backend's jobs).
#
# Env: PGHOST, PGUSER, PGDATABASE and PGPASSWORD (libpq), or PGPASSWORD_FILE
#      to read the password from a secret file; VM_HOST (default
#      victoriametrics), VM_PORT (default 8428).
set -uo pipefail

if [[ -n ${PGPASSWORD_FILE:-} ]]; then
  PGPASSWORD=$(<"$PGPASSWORD_FILE")
  export PGPASSWORD
fi

vm_host=${VM_HOST:-victoriametrics}
vm_port=${VM_PORT:-8428}
interval=86400
heartbeat=3600

log() { printf '%s analytics-retention: %s\n' "$(date -u +%FT%TZ)" "$*"; }

# POST one sample in Prometheus text format to VictoriaMetrics. No curl in this
# image, so plain HTTP over bash's /dev/tcp. Failing to push is logged, never fatal.
push() {
  local body
  body="analytics_retention_last_success_timestamp_seconds $1"$'\n'
  if ! { exec 3<>"/dev/tcp/$vm_host/$vm_port"; } 2>/dev/null; then
    log "metric not pushed: can't reach $vm_host:$vm_port (observability profile not running?)"
    return 0
  fi
  printf 'POST /api/v1/import/prometheus HTTP/1.1\r\nHost: %s\r\nContent-Type: text/plain\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s' \
    "$vm_host" "${#body}" "$body" >&3
  local status
  IFS= read -r -t 10 status <&3 || status='no response'
  exec 3>&-
  [[ $status == *' 204'* ]] || log "metric push answered: ${status%$'\r'}"
}

last_success=$(date +%s)
next_run=$last_success
while true; do
  now=$(date +%s)
  if ((now >= next_run)); then
    if psql -X -q -v ON_ERROR_STOP=1 -f /retention.sql; then
      last_success=$(date +%s)
      log "deleted analytics data older than 90 days"
    else
      log "run FAILED; retrying in an hour"
    fi
    # A failed run retries at the next heartbeat, a good one waits a day.
    if ((last_success >= now)); then next_run=$((now + interval)); else next_run=$((now + heartbeat)); fi
  fi
  push "$last_success"
  sleep "$heartbeat"
done
