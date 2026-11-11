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
| `.secrets/`                                   | **never**        | The above, decrypted on the host (gitignored)          |
| `.env`                                        | on the host only | `DOMAIN`, `BACKEND_IMAGE`, `WEB_IMAGE` (digests)       |

Who can decrypt what, and how to add people or hosts, is in
[`../secrets/README.md`](../secrets/README.md). The host needs its own age key
listed under `machines:` there with `env: prod`.

`backend.env` must set `DATABASE_URL=postgres://overkill_todo:<db_password>@db:5432/overkill_todo`,
`DEV_MODE=false`, real SMTP and Turnstile settings, and fresh JWT, CSRF and
encryption keys. The backend refuses to start in production with a no-op adapter
or missing config, which is the point: a bad deploy fails at boot, not later.

Rate limiting of the auth endpoints (`/api/v1/auth/*`, per client IP) is done at
the Traefik level, not by this Caddy edge or the backend. Without it, password
login has no brute-force throttle beyond Turnstile.

## First deploy

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
```

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

**Disk.** VictoriaLogs is capped by `VLOGS_MAX_DISK` (default 10 GiB, set in
`.env`); Tempo and VictoriaLogs data live in the `tempo-data` and `vlogs-data`
volumes. Alerts on disk use arrive in a later slice.

## Not covered here

- Host hardening (SSH keys only, firewall to 22/80/443, unattended upgrades).
- Log shipping and alerting. Caddy and the backend log JSON to stdout; ship
  `docker logs` to your log store and add an external uptime check on
  `/api/v1/health/ready`.
