# Rust + Svelte starter

A full-stack starting point with a finished auth slice, so a new project starts
at its first real feature instead of at login.

- **Backend:** Rust 2024, Axum 0.8, sqlx + Postgres 18, hexagonal layout
  ([backend/README.md](backend/README.md))
- **Frontend:** Astro 7 (static) with Svelte 5 islands, Tailwind 4,
  shadcn-svelte (`apps/web`)
- **Auth:** email + password (Argon2id), email verification, password reset,
  Google OAuth, Turnstile, RS256 JWT + rotating refresh tokens, CSRF, roles,
  admin user list, account export and scheduled deletion
- **Tests:** unit (mockall, proptest), integration (`#[sqlx::test]`), Vitest
  browser-mode components, Playwright e2e (mocked API), real-stack e2e, smoke
- **Ops:** Docker images, Compose for local and single-host production,
  optional Kubernetes base, GitHub Actions CI, Dependabot

## Start a project from it

```bash
cp -r ~/Work/dev-workflows/starters/rust-svelte ~/Work/myproject
cd ~/Work/myproject && rm -rf node_modules backend/target && git init
```

Then rename: `app` / `@app/*` / `App` are placeholders (package names, Compose
project, DB name, JWT issuer and audience in `backend/src/infrastructure/api/jwt.rs`,
`APP_NAME`, page titles, the leader-lock key in `main.rs`).

```bash
grep -rIl --exclude-dir={node_modules,target,.sqlx,dist} -e '\bapp\b' -e '@app/' -e 'App\b' .
```

## Daily use

```bash
just setup   # once: tools (mise), deps, browsers, backend/.env, database
just dev     # backend :4322 + web :4321 (open this one; it proxies /api)
just         # list every recipe
just ci      # everything CI checks, locally
```

Make yourself an admin after signing up: `just grant-admin you@example.com`.
In dev, verification codes print in the backend log.

| Recipe | What it runs |
| --- | --- |
| `just test` / `just test-integration` | backend unit / DB-backed tests |
| `just test-web` | `astro check` + Vitest (real Chromium) |
| `just e2e` | Playwright against the built site, API mocked |
| `just e2e-integration` | Playwright signup against `just dev` |
| `just smoke <url>` | read-only checks against a deployed site |
| `just up` | prod-like stack in Docker behind Caddy on :8080 |

## Layout

```
backend/            Rust API (see backend/README.md, backend/ARCHITECTURE.md)
apps/web/           Astro + Svelte site, nginx image
packages/           shared design tokens
deploy/compose/     single-host production (Caddy TLS, digests, nightly dumps)
deploy/k8s/         optional kustomize base (CNPG Postgres, Ingress)
scripts/            dev env generation
.github/            CI and Dependabot
```

## Conventions

- **Commits:** Conventional Commits, enforced by commitlint (`feat:`, `fix:`,
  `chore:` …). Pre-commit hooks are path-scoped and fast, so don't skip them.
- **Branches:** short-lived branches into `main`; `main` is always deployable.
- **Dependencies:** pnpm refuses releases younger than 24h; Dependabot waits 3
  days; `cargo deny` and `cargo audit` run in CI.
- **Database:** forward-only migrations, expand then contract; let Postgres
  enforce invariants (constraints, partial unique indexes).
- **Deploys:** an image digest change committed to the deploy config. CI builds
  images; it never holds production credentials.

## Where this came from

Extracted from a production project and trimmed to the auth core. The reasoning
behind most choices lives in the notes vault: start at *Starting a new project*.
