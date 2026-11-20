# Single-host deployment (Docker Compose)

The default way to run this in production: one VPS, the same images CI builds,
the same single-origin routing as local (`/api` → backend, everything else → web).
Move to Kubernetes only when the workload needs it (see `../k8s/README.md`).

> **Status:** the local `full` profile is exercised by the template. This
> production override is validated with `docker compose config` but has not run a
> real deployment yet. Treat the first deploy as the test.

## Files

| File                                          | Committed        | What                                                   |
| --------------------------------------------- | ---------------- | ------------------------------------------------------ |
| `compose.prod.yaml`                           | yes              | Override on top of `../../compose.yaml`                |
| `Caddyfile`                                   | yes              | TLS edge for `$DOMAIN` (automatic certificates)        |
| `../secrets/prod/backend.sops.env`            | yes, encrypted   | Backend config, the contract is `backend/.env.example` |
| `../secrets/prod/db_password.sops`            | yes, encrypted   | Postgres password                                      |
| `../secrets/prod/grafana_admin_password.sops` | yes, encrypted   | Grafana `admin` password (observability)               |
| `../secrets/prod/analytics_db_password.sops`  | yes, encrypted   | Umami's own Postgres password, hex (analytics)         |
| `../secrets/prod/umami_app_secret.sops`       | yes, encrypted   | Umami `APP_SECRET` (analytics)                         |
| `.secrets/`                                   | **never**        | The above, decrypted on the host (gitignored)          |
| `.env`                                        | on the host only | `DOMAIN`, `BACKEND_IMAGE`, `WEB_IMAGE` (digests)       |

Who can decrypt what, and how to add people or hosts, is in
[`../secrets/README.md`](../secrets/README.md). The host needs its own age key
listed under `machines:` there with `env: prod`, generated on the host with
`age-keygen -pq` (post-quantum keys only; `just secrets-sync` refuses
classical ones).

`backend.env` must set `DATABASE_URL=postgres://overkill_todo:<db_password>@db:5432/overkill_todo`,
`DEV_MODE=false`, real SMTP and Turnstile settings, and fresh token, CSRF and
encryption keys (`JWT_SIGNING_KEY` from `just jwt-keygen`; rotation steps are
in `backend/.env.example`). The backend refuses to start in production with a no-op adapter
or missing config, which is the point: a bad deploy fails at boot, not later.

Nothing rate-limits the auth endpoints yet: not this Caddy edge, not the
backend. Per-address and per-account limits on `/api/v1/auth/*` are planned for
the production edge (Traefik, see Security below). Until then, password login
has no brute-force throttle beyond Turnstile and the Argon2 cost. The auth
alerts (Observability) show an attack, but don't stop it.

### TLS key exchange (post-quantum)

Whatever terminates public TLS (this Caddy edge now, Traefik later, or a CDN
in front) must offer hybrid post-quantum key exchange, `X25519MLKEM768`,
first, and still accept classical-only clients (OT-009). Traffic recorded
today can otherwise be decrypted later by a quantum computer.

- **Caddy** (Go ≥ 1.24): the default. Don't set `curves` in the `tls` block,
  or list `x25519mlkem768` first.
- **Traefik** (Go ≥ 1.24): the default. Leave `curvePreferences` unset in
  the TLS options, or list `X25519MLKEM768` first.
- **A CDN in front** (e.g. Cloudflare proxying the hostname): the browser leg
  is the CDN's. Check it offers the hybrid, and apply the same rule to the
  CDN→origin leg.

`just smoke https://<domain>` checks both halves against the live edge
(`scripts/smoke-tls.sh`, needs OpenSSL ≥ 3.5) and fails if either is
missing. The backend's own outgoing TLS (Google, Turnstile, SMTP) is pinned
in code and covered by `just test-integration`.

## First deploy

Before the first deploy, on a machine cleared for prod (OT-009):

- Give the host a post-quantum age key (`age-keygen -pq` on the host), add it
  under `machines:` in `deploy/secrets/access.yaml`, `just secrets-sync`, commit.
- Put a fresh token key in the prod backend env: `JWT_SIGNING_KEY` from
  `just jwt-keygen`, with no `JWT_PRIVATE_KEY`/`JWT_PUBLIC_KEY` (the backend
  refuses to start with them) and `JWT_TRUSTED_KEYS` empty.
- Make sure the edge offers post-quantum key exchange (see
  [TLS key exchange](#tls-key-exchange-post-quantum)); `just smoke` below
  checks it.

```bash
# on the host, in a checkout of the repo
just secrets-decrypt prod        # → deploy/compose/.secrets/ (needs the host's age key)

cat > .env <<'ENV'
DOMAIN=app.example.com
BACKEND_IMAGE=ghcr.io/<owner>/<repo>/backend@sha256:...
WEB_IMAGE=ghcr.io/<owner>/<repo>/web@sha256:...
ENV

alias dc='docker compose -f compose.yaml -f deploy/compose/compose.prod.yaml --profile full'
dc up -d
curl -fsS https://app.example.com/api/v1/health/ready
just smoke https://app.example.com   # includes the post-quantum key-exchange check
# The API description is dev-only (OT-002): the backend must answer 404 for both.
# The edge sends these paths to web, so ask the backend directly (scratch image,
# no curl inside: borrow its network namespace).
for p in /docs /api-doc/openapi.json; do
  docker run --rm --network "container:$(dc ps -q backend)" curlimages/curl \
    -s -o /dev/null -w "$p %{http_code}\n" "http://localhost:4322$p"
done
```

Web analytics (OT-005) on the first deploy, after the app is up (details in
[Web analytics](#web-analytics)):

1. Build the [edge contract](#edge-contract) into the Traefik edge, then run
   `scripts/check-analytics-visitors.sh https://<domain>` against it: 2
   Visitors, no stored address. Confirm `https://<domain>/stats/` and
   `/stats/login` answer 404 from outside.
2. Start `analytics-db` and `umami`, change Umami's `admin` password, create
   the site and set `PUBLIC_UMAMI_WEBSITE_ID` ([Turning it on](#turning-it-on)).
   Then `just smoke https://<domain>`.
3. After a day, compare Umami's Visitors with Cloudflare Web Analytics for the
   same day. They should be within about 20%. If Umami is far lower, the client
   address isn't getting through (re-run step 1).
4. Sign up once with a real address in a normal browser, then build the funnel
   in Umami (Path `/register` → Event `signup-submitted` → Event
   `email-verified`). It should show your sign-up completed. Delete the test
   account afterwards.
5. Confirm retention ran: `dc logs analytics-retention` shows "deleted
   analytics data older than 90 days", and in Grafana Explore
   `analytics_retention_last_success_timestamp_seconds` has a value (it shows
   up about a minute after the push).

Auth anomaly metrics (OT-007) on the first deploy: the alert rules and the
"Auth security" dashboard are config, so restart them before rolling the
backend (`dc restart vmalert grafana`). Rules over metrics that don't exist yet
stay quiet. Then roll the backend digest, `just smoke https://<domain>`, sign
in once, and check that the dashboard's successful sign-ins rise by one within a
minute. Sign in once with a wrong password, and check that "Failed sign-ins per
account" lists your account with a working **View trace** link. Send one
sign-in with no bot-check token (for example with `curl`), and check that it
gets `400 bot_check_failed` and that "Bot-check rejections by form" rises for
`sign_in`. Record each auth alert that fires in the first month, and
whether it was a real attack, in the OT-007 design's Hypothesis.

Browser errors and performance (OT-006) on the first deploy:

1. Add a Cloudflare rate-limiting rule for `/api/v1/client-telemetry` per
   client IP. Until then the backend's per-replica budget
   (`CLIENT_TELEMETRY_MAX_EVENTS_PER_SEC`) is the only limit: a flood can crowd
   out real reports but can't grow storage or series.
2. Build the production web image with `RELEASE=<sha>` (CI passes the commit
   SHA), so reports carry the release and `just decode-stack <sha>` finds its
   `web-sourcemaps-<sha>` artifact.
3. Set `PUBLIC_RUM_SAMPLE_RATE` for production (unset means every page load;
   lower it once traffic grows). Errors and page loads are never sampled.
4. Roll the config (`dc restart vmalert grafana`) and the images, run
   `just smoke https://<domain>`, then throw a deliberate error on a staging
   page (`setTimeout(() => { throw new Error('ot006 check') })` in the
   console) and check that it reaches the Frontend dashboard within a minute
   and that `just decode-stack <sha>` turns its stack into `src/…` lines.

Security (OT-010), before launch:

1. Add sign-in rate limiting at the edge (Traefik), per address and per
   account. Until then, threat model P-03 is an accepted gap: bot protection
   and Argon2 cost are the only brakes on password guessing.
2. Commission an external penetration test of the deployed app, scoped by
   [`docs/security/threat-model.md`](../../docs/security/threat-model.md): its
   attackers, its numbered properties, and above all its accepted gaps and
   stated assumptions (TLS, the modelled browser, the edge), which no proof
   here covers. Record the findings against property numbers, and add a
   property for anything the catalogue missed.

The backend runs its embedded migrations at startup (`run_migrations` in
`main.rs`). Postgres serialises concurrent `sqlx` migrators with an advisory
lock, so this stays safe if you later run more than one replica.

## Deploying a new version

A deploy is a commit that changes the digests in `.env` (keep a copy of `.env`
in the repo's deploy history, or in a tiny private repo). CI builds
`sha-<commit>` images; resolve the digest and pin it:

```bash
docker buildx imagetools inspect ghcr.io/<owner>/<repo>/backend:sha-<commit> | awk '/^Digest:/ {print $2}'
dc pull && dc up -d && just smoke https://app.example.com
```

Rollback is the same command with the previous digests. Migrations are
forward-only, so a rollback must tolerate the newer schema: make schema changes
additive first (expand), deploy, then remove the old shape later (contract).

## Backups and restore

The `backup` service writes a nightly `pg_dump -Fc` to `deploy/compose/backups/`
and keeps 14. **That is not a backup until it leaves the host**: sync the folder
offsite (restic or rclone to object storage) on a timer.

Restore rehearsal, before the first real user and then quarterly:

```bash
dc stop backend
dc exec -T db dropdb -U overkill_todo overkill_todo && dc exec -T db createdb -U overkill_todo overkill_todo
dc exec -T db pg_restore -U overkill_todo -d overkill_todo --no-owner < deploy/compose/backups/<file>.dump
dc start backend
```

Time it and write the number down. See `dev-workflows/runbooks/dr-rehearsal.md`.

## Observability

Traces (Tempo), logs (VictoriaLogs) and Grafana run on the same host, under the
`observability` profile. Config lives in `deploy/observability/`; the design is
`openspec/changes/ot-001-observability/design.md` (archived to `openspec/specs/`
once done). Everything is kept 30 days.

**Deploy order matters.** A backend with `DEV_MODE=false` refuses to start
without `OTEL_EXPORTER_OTLP_ENDPOINT` (set in `compose.prod.yaml`), so bring the
stack up first, then roll the backend digest:

```bash
just secrets-decrypt prod        # includes grafana_admin_password

alias dc='docker compose -f compose.yaml -f deploy/compose/compose.prod.yaml --profile full --profile observability'
dc up -d otel-collector tempo victorialogs grafana   # 1. the stack
dc up -d                                            # 2. then the new backend digest
just smoke https://app.example.com
```

**Config changes need a restart.** The configs are bind-mounted files, and
`dc up -d` doesn't recreate a container when only a mounted file changed. vmalert
reads its rules only at startup. After changing `deploy/observability/**` or a
`Caddyfile`, restart the affected services, for example
`dc restart vmalert otel-collector edge`.

**Access.** Nothing observability-related is routed by Caddy or published on a
public port. Grafana listens on the host's loopback only:

```bash
ssh -L 3000:localhost:3000 <host>    # then open http://localhost:3000, user admin
```

In Explore, an expanded log line has **Links → trace_id → View trace**; in a
trace, a span's **Related logs** button shows that trace's log lines (backend and
edge).

**Rollback.** Roll the backend digest back as usual; an older backend ignores the
telemetry variables. The observability services can be stopped at any time
(`dc stop otel-collector tempo victorialogs grafana`) without affecting the app:
the backend keeps serving and logs a warning that telemetry isn't delivered.

### Auth security (OT-007)

The backend counts every credential and code check, and the **Auth security**
dashboard (`/d/auth-security`) shows them. The design is
`openspec/changes/archive/2026-11-19-ot-007-auth-anomaly-metrics/design.md`.

Metrics carry no email, user ID, address, code or token. Every label comes
from a closed set, so the series count is fixed:

| Metric (VictoriaMetrics name)     | Labels                                                                                                                       |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `auth_attempts_total`             | `flow` = `password_sign_in`, `email_code`, `reset_code`, `magic_link`; `outcome` = `success`, `failure`, `locked`, `expired` |
| `auth_code_lockouts_total`        | `flow` = `email_code`, `reset_code`. Counted once per code, on the guess that uses the last attempt                          |
| `auth_bot_check_rejections_total` | `form` = `register`, `sign_in`, `resend_verification`, `forgot_password`; `reason` = `missing`, `invalid`                    |
| `auth_session_anomalies_total`    | `kind` = `token_reuse`, `device_mismatch`                                                                                    |

An unknown email counts as a failed sign-in, the same as a wrong password. A
Turnstile outage is a 500, not a rejection.

To show which account was affected, the backend also writes log events
(VictoriaLogs, field `event`). Each has the account's `user_id` and the
request's `trace_id`, and never an email, code, token, password or address:

| `event`                    | Level | When                                                                                           |
| -------------------------- | ----- | ---------------------------------------------------------------------------------------------- |
| `auth.code.locked`         | warn  | An email or reset code locks (`flow`)                                                          |
| `auth.code.refused_locked` | info  | A check is refused because the code is locked (`flow`, including `magic_link`)                 |
| `auth.sign_in.failed`      | info  | A wrong password, or a sign-in to an account with no password. An unknown email writes nothing |

The dashboard turns these into "Recent lockouts" (open an entry, then
**View trace**) and per-account tables of lockouts, refused checks and failed
sign-ins. Many accounts with a few failures each looks like credential
stuffing, and one account with many looks like a targeted guess.

Alerts (vmalert group `auth`, `severity: warning`, email like the rest). Each
email links to its panel, and a resolved notice follows when the count drops:

| Alert                        | Fires when                                                 | Links to                     |
| ---------------------------- | ---------------------------------------------------------- | ---------------------------- |
| `AuthSignInFailuresHigh`     | more than 30 failed password sign-ins in 15 minutes        | Failed sign-ins per account  |
| `AuthCodeLockoutsHigh`       | more than 5 codes locked in an hour, all flows             | Recent lockouts              |
| `AuthBotCheckRejectionsHigh` | more than 50 bot-check rejections in 15 minutes, all forms | Bot-check rejections by form |
| `AuthSessionAnomalies`       | 3 or more session anomalies in an hour                     | Session anomalies            |

The thresholds are first guesses, made with no production traffic. Record each
firing in the first month, and whether it was real, in the OT-007 Hypothesis.
Then revise them in `deploy/observability/alerts/overkill-todo.yml`, whose unit
tests (`just obs-check`) pin each boundary.

Locally, `just load auth-abuse` fires the sign-in, lockout and session alerts,
and `just load auth-mix` fires none. Alertmanager's local route has no receiver,
so no email is sent.

**Manual check: bot-check rejections.** Turnstile is off in `DEV_MODE`, so no
local run counts a rejection. To check the counting against the real service,
run a non-dev backend with Cloudflare's always-fail test secret
(`TURNSTILE_SECRET_KEY=2x0000000000000000000000000000000AA`). Then sign in
through the web app: the request gets `400 bot_check_failed`, and
`auth_bot_check_rejections_total{form="sign_in",reason="invalid"}` goes up by 1.
Submit a request with no token, and `reason="missing"` goes up.

**Disk.** VictoriaLogs is capped by `VLOGS_MAX_DISK` (default 10 GiB, set in
`.env`); Tempo and VictoriaLogs data live in the `tempo-data` and `vlogs-data`
volumes. Alerts on disk use arrive in a later slice.

## Web analytics

Cookie-free page views (and, later, sign-up events) in a self-hosted Umami
with its own Postgres, under the `analytics` profile (OT-005; design in
`openspec/changes/ot-005-web-analytics/design.md`). It's off until
`PUBLIC_UMAMI_WEBSITE_ID` is set on `web`.

### Edge contract

Whatever the edge is (this Caddy now, Traefik in production), it must:

1. **Route two paths only** to Umami, `/stats` prefix removed:
   `/stats/script.js` → `umami:3000/script.js` and `/stats/api/send` →
   `umami:3000/api/send`. Anything else under `/stats/` answers **404**, so
   Umami's login and reports are never public. `/api/*` and every other route
   stay as they are.
2. **Pass the real client address** in `X-Analytics-Client-IP` (Umami's
   `CLIENT_IP_HEADER`), overwriting any copy the client sent. Forwarded
   headers (`CF-Connecting-IP`, `X-Forwarded-For`) are believed only from
   known proxies (the CDN's published ranges), otherwise the connection's
   address is used. Get this wrong and every Visitor collapses into one.
3. **Drop `Cookie` and `Authorization`** on the way to Umami. The browser
   sends the session cookie with the script load, and Umami needs none of it.

**Caddy** (`Caddyfile`, `Caddyfile.local`): the global
`servers { trusted_proxies …; trusted_proxies_strict; client_ip_headers
CF-Connecting-IP X-Forwarded-For }` block (Cloudflare's ranges in production,
dated, private ranges locally), `request_header -X-Analytics-Client-IP` for
every request, and a `handle_path /stats/*` block ahead of the catch-all with
`header_up X-Analytics-Client-IP {client_ip}`, `header_up -Cookie` and
`header_up -Authorization`.

**Traefik** (to build with the production setup):

- Entrypoint `forwardedHeaders.trustedIPs` = the CDN's ranges, so Traefik keeps
  `X-Forwarded-For` only from them and rewrites it otherwise. If Cloudflare is
  in front, also firewall the origin to Cloudflare's ranges (or use a
  Cloudflare-aware real-IP plugin), so the forwarded address is trustworthy.
- A router ``PathPrefix(`/stats/`) && (Path(`/stats/script.js`) ||
Path(`/stats/api/send`))``, priority above the web catch-all, to the Umami
  service, with middlewares: `stripPrefix` (`/stats`), and a `headers`
  middleware removing `Cookie` and `Authorization`
  (`customRequestHeaders: {Cookie: "", Authorization: ""}`).
- The client address header: either a real-IP middleware that sets
  `X-Analytics-Client-IP` from the trusted address, or set Umami's
  `CLIENT_IP_HEADER=x-forwarded-for` and `TRUSTED_PROXIES` to Traefik's own
  network so Umami takes the right hop (Umami walks the list right to left).
- A second router ``PathPrefix(`/stats/`)`` to nothing (Traefik's 404), or no
  route at all, so the rest of `/stats/` falls through to a 404.

Check it with `scripts/check-analytics-visitors.sh https://<domain>` from a host
the edge trusts (it must report 2 Visitors and no stored address) and
`curl -o /dev/null -w '%{http_code}' https://<domain>/stats/login` (404).

### Turning it on

```bash
just secrets-decrypt prod        # includes analytics_db_password, umami_app_secret

alias dc='docker compose -f compose.yaml -f deploy/compose/compose.prod.yaml --profile full --profile analytics'
dc up -d analytics-db umami analytics-retention
ssh -L 3001:localhost:3001 <host>   # from your machine; then open http://localhost:3001
```

1. Sign in as `admin` / `umami` and **change the password at once**
   (Settings → Profile). Umami is only on the host's loopback, but the default
   is public knowledge.
2. Settings → Websites → Add website, with the production domain. Copy its
   website ID.
3. Add `PUBLIC_UMAMI_WEBSITE_ID=<id>` to `.env`, then `dc up -d web`.
4. Open the site in a browser without Do Not Track or Global Privacy Control:
   the page view appears in Umami within a minute.

**Rollback:** remove `PUBLIC_UMAMI_WEBSITE_ID` from `.env` and `dc up -d web`.
The site stops loading the tracker at once. Then
`dc stop umami analytics-retention analytics-db` if you like; the `/stats/*`
routes just answer 404 or 502.

**Kept 90 days.** The `analytics-retention` service runs
`deploy/analytics/retention.sql` once a day: it deletes page views, events and
their data older than 90 days, then sessions left without events, in one
transaction. Run it by hand with
`dc exec analytics-retention psql -X -f /retention.sql`. After each successful
run, and every hour, it pushes
`analytics_retention_last_success_timestamp_seconds` to VictoriaMetrics, so it
needs the `observability` profile for its alert. **AnalyticsRetentionNotSucceeding**
emails the operator when no run has succeeded for 2 days and 15 minutes,
including when the container has stopped. After upgrading Umami, run
`just analytics-retention-check` locally: it runs the SQL against a seeded
copy of the new schema.

**Not backed up.** Umami's data lives in the `analytics-db-data` volume and is
deliberately left out of the nightly dump. Losing it loses analytics history
only. Accounts and the app are unaffected.

## Not covered here

- Host hardening (SSH keys only, firewall to 22/80/443, unattended upgrades).
- Log shipping and alerting. Caddy and the backend log JSON to stdout; ship
  `docker logs` to your log store and add an external uptime check on
  `/api/v1/health/ready`.
