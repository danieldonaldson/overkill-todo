#!/usr/bin/env bash
# OT-005 retention check: runs deploy/analytics/retention.sql against a scratch
# copy of the live Umami schema, seeded with data 89 and 91 days old, and checks
# that exactly the old data (and sessions left without events) is deleted.
# Re-run after any Umami upgrade: the SQL names Umami's tables and columns.
#
# Needs the analytics profile running (`just analytics-up`). Touches only a
# scratch database, created and dropped here.
#
# Env: ANALYTICS_DB_EXEC  command prefix that runs a client tool inside the
#      analytics database container
#      (default: docker compose exec -T analytics-db)
set -euo pipefail

exec_cmd=${ANALYTICS_DB_EXEC:-docker compose exec -T analytics-db}
scratch=retention_check
sql_file=$(dirname "$0")/../deploy/analytics/retention.sql

fail() { echo "FAIL: $*" >&2; exit 1; }
psql_in() { $exec_cmd psql -X -q -v ON_ERROR_STOP=1 -U umami "$@"; }

$exec_cmd dropdb -U umami --if-exists "$scratch" 2>/dev/null
$exec_cmd createdb -U umami "$scratch"
trap '$exec_cmd dropdb -U umami --if-exists "$scratch"' EXIT
$exec_cmd pg_dump -U umami -d umami --schema-only --no-owner | psql_in -d "$scratch" >/dev/null

# Sessions: old (only old events), mixed (old start, one recent event), new.
# Events at 91 days (must go) and 89 days (must stay), with event and session data.
psql_in -d "$scratch" <<'SQL'
INSERT INTO website (website_id, name) VALUES ('00000000-0000-0000-0000-0000000000aa', 'check');
INSERT INTO session (session_id, website_id, created_at) VALUES
  ('00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-0000000000aa', now() - interval '100 days'),
  ('00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-0000000000aa', now() - interval '100 days'),
  ('00000000-0000-0000-0000-000000000003', '00000000-0000-0000-0000-0000000000aa', now() - interval '89 days');
INSERT INTO website_event (event_id, website_id, session_id, visit_id, url_path, created_at) VALUES
  ('00000000-0000-0000-0000-0000000000e1', '00000000-0000-0000-0000-0000000000aa', '00000000-0000-0000-0000-000000000001', '00000000-0000-0000-0000-0000000000f1', '/old',   now() - interval '91 days'),
  ('00000000-0000-0000-0000-0000000000e2', '00000000-0000-0000-0000-0000000000aa', '00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-0000000000f2', '/old',   now() - interval '91 days'),
  ('00000000-0000-0000-0000-0000000000e3', '00000000-0000-0000-0000-0000000000aa', '00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-0000000000f3', '/keep',  now() - interval '89 days'),
  ('00000000-0000-0000-0000-0000000000e4', '00000000-0000-0000-0000-0000000000aa', '00000000-0000-0000-0000-000000000003', '00000000-0000-0000-0000-0000000000f4', '/keep',  now() - interval '89 days');
INSERT INTO event_data (event_data_id, website_id, website_event_id, data_key, data_type, created_at) VALUES
  ('00000000-0000-0000-0000-0000000000d1', '00000000-0000-0000-0000-0000000000aa', '00000000-0000-0000-0000-0000000000e1', 'via', 1, now() - interval '91 days'),
  ('00000000-0000-0000-0000-0000000000d3', '00000000-0000-0000-0000-0000000000aa', '00000000-0000-0000-0000-0000000000e3', 'via', 1, now() - interval '89 days');
INSERT INTO session_data (session_data_id, website_id, session_id, data_key, data_type, created_at) VALUES
  ('00000000-0000-0000-0000-0000000000c1', '00000000-0000-0000-0000-0000000000aa', '00000000-0000-0000-0000-000000000001', 'k', 1, now() - interval '91 days'),
  ('00000000-0000-0000-0000-0000000000c3', '00000000-0000-0000-0000-0000000000aa', '00000000-0000-0000-0000-000000000003', 'k', 1, now() - interval '89 days');
SQL

psql_in -d "$scratch" <"$sql_file"

remaining() { psql_in -d "$scratch" -At -c "SELECT string_agg(right($2::text, 2), ',' ORDER BY $2) FROM $1"; }
check() { # <table> <id column> <expected ids, last 2 hex digits>
  local got
  got=$(remaining "$1" "$2")
  [[ $got == "$3" ]] || fail "$1: expected [$3] to remain, got [$got]"
  echo "ok  $1: [$got] remain"
}
check website_event event_id e3,e4
check event_data event_data_id d3
check session session_id 02,03
check session_data session_data_id c3
check website website_id aa
echo "PASS"
