#!/usr/bin/env bash
# OT-005 hypothesis check: analytics served through the edge still tells
# Visitors apart, and no client address is stored.
#
# Sends Umami collect requests through the edge's /stats/api/send as if from
# three requests: two from 203.0.113.10 (one also forging the header Umami
# reads, which the edge must overwrite) and one from 198.51.100.20, all with
# the same browser. Passes if Umami counts exactly 2 Visitors and neither
# address appears anywhere in the analytics database.
#
# The edge must believe X-Forwarded-For from wherever this runs (locally the
# Docker bridge is a trusted private range). Edge-agnostic: re-run it against
# the Traefik edge on first deploy, from a host the edge trusts.
#
# Usage: check-analytics-visitors.sh [edge-url]       (default http://localhost:${EDGE_PORT:-8080})
# Env:   UMAMI_URL       operator URL of Umami         (default http://localhost:${UMAMI_PORT:-3001})
#        UMAMI_USER / UMAMI_PASSWORD                   (default admin / umami: a fresh local install)
#        ANALYTICS_PG_DUMP  command printing the analytics DB as SQL
#                       (default: pg_dump in the local analytics-db container)
set -euo pipefail

edge=${1:-http://localhost:${EDGE_PORT:-8080}}
edge=${edge%/}
umami=${UMAMI_URL:-http://localhost:${UMAMI_PORT:-3001}}
pg_dump_cmd=${ANALYTICS_PG_DUMP:-docker compose exec -T analytics-db pg_dump -U umami -d umami --data-only}
ua='Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36'
ip_a=203.0.113.10
ip_b=198.51.100.20
forged=192.0.2.99

fail() { echo "FAIL: $*" >&2; exit 1; }

token=$(curl -fsS "$umami/api/auth/login" -H 'content-type: application/json' \
  -d "$(jq -n --arg u "${UMAMI_USER:-admin}" --arg p "${UMAMI_PASSWORD:-umami}" '{username: $u, password: $p}')" |
  jq -r .token) || fail "can't sign in to Umami at $umami"
auth=(-H "authorization: Bearer $token")

site=$(curl -fsS "${auth[@]}" "$umami/api/websites" -H 'content-type: application/json' \
  -d "{\"name\": \"hypothesis-check-$(date +%s)\", \"domain\": \"hypothesis-check.invalid\"}" | jq -r .id)
cleanup() { curl -fsS -X DELETE "${auth[@]}" "$umami/api/websites/$site" >/dev/null || true; }
trap cleanup EXIT
echo "Test site $site; sending through $edge/stats/api/send"

send() { # <x-forwarded-for> <path> [extra curl args...]
  local xff=$1 path=$2
  shift 2
  local body
  body=$(jq -n --arg w "$site" --arg u "https://hypothesis-check.invalid$path" \
    '{type: "event", payload: {website: $w, hostname: "hypothesis-check.invalid", url: $u,
      referrer: "", language: "en-US", screen: "1920x1080", title: "check"}}')
  local code
  code=$(curl -sS -o /dev/null -w '%{http_code}' "$edge/stats/api/send" \
    -H 'content-type: application/json' -H "user-agent: $ua" -H "x-forwarded-for: $xff" "$@" \
    -d "$body")
  [[ $code == 200 ]] || fail "collect from $xff returned $code"
}

send "$ip_a" /login
send "$ip_a" /register -H "x-analytics-client-ip: $forged"
send "$ip_b" /login

start=$((($(date +%s) - 3600) * 1000))
visitors=
for _ in $(seq 1 20); do
  stats=$(curl -fsS "${auth[@]}" "$umami/api/websites/$site/stats?startAt=$start&endAt=$(($(date +%s) * 1000 + 60000))")
  visitors=$(jq -r '.visitors | if type == "object" then .value else . end' <<<"$stats")
  pageviews=$(jq -r '.pageviews | if type == "object" then .value else . end' <<<"$stats")
  [[ $pageviews == 3 ]] && break
  sleep 0.5
done
echo "Umami counted $pageviews page views from $visitors Visitors"
[[ $pageviews == 3 ]] || fail "expected 3 page views, got $pageviews"
[[ $visitors == 2 ]] || fail "expected 2 Visitors (two networks), got $visitors: the client address isn't reaching Umami"

dump=$($pg_dump_cmd) || fail "couldn't dump the analytics database ($pg_dump_cmd)"
for ip in "$ip_a" "$ip_b" "$forged"; do
  grep -qF "$ip" <<<"$dump" && fail "client address $ip is stored in the analytics database"
done
echo "No client address stored in the analytics database"
echo "PASS"
