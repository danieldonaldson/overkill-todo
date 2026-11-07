# Single-host deployment (Docker Compose)

The default way to run this in production: one VPS, the same images CI builds,
the same single-origin routing as local (`/api` → backend, everything else → web).
Move to Kubernetes only when the workload needs it (see `../k8s/README.md`).

> **Status:** the local `full` profile is exercised by the template. This
> production override is validated with `docker compose config` but has not run a
> real deployment yet. Treat the first deploy as the test.

## Files

| File | Committed | What |
| --- | --- | --- |
| `compose.prod.yaml` | yes | Override on top of `../../compose.yaml` |
| `Caddyfile` | yes | TLS edge for `$DOMAIN` (automatic certificates) |
| `backend.sops.env` | yes, encrypted | Backend config, the contract is `backend/.env.example` |
| `db_password.sops` | yes, encrypted | Postgres password |
| `backend.env`, `db_password` | **never** | Decrypted on the host at deploy time (gitignored) |
| `.env` | on the host only | `DOMAIN`, `BACKEND_IMAGE`, `WEB_IMAGE` (digests) |

`backend.env` must set `DATABASE_URL=postgres://app:<db_password>@db:5432/app`,
`DEV_MODE=false`, real SMTP and Turnstile settings, and fresh JWT, CSRF and
encryption keys. The backend refuses to start in production with a no-op adapter
or missing config, which is the point: a bad deploy fails at boot, not later.

## First deploy

```bash
# on the host, in a checkout of the repo
sops -d deploy/compose/backend.sops.env > deploy/compose/backend.env
sops -d deploy/compose/db_password.sops > deploy/compose/db_password
chmod 600 deploy/compose/backend.env deploy/compose/db_password

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
dc exec -T db dropdb -U app app && dc exec -T db createdb -U app app
dc exec -T db pg_restore -U app -d app --no-owner < deploy/compose/backups/<file>.dump
dc start backend
```

Time it and write the number down. See `dev-workflows/runbooks/dr-rehearsal.md`.

## Not covered here

- Host hardening (SSH keys only, firewall to 22/80/443, unattended upgrades).
- Log shipping and alerting. Caddy and the backend log JSON to stdout; ship
  `docker logs` to your log store and add an external uptime check on
  `/api/v1/health/ready`.
