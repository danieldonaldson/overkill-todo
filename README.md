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

Then rename: `app` / `@overkill-todo/*` / `App` are placeholders (package names, Compose
project, DB name, JWT issuer and audience in `backend/src/infrastructure/api/jwt.rs`,
`APP_NAME`, page titles, the leader-lock key in `main.rs`).

```bash
grep -rIl --exclude-dir={node_modules,target,.sqlx,dist} -e '\bapp\b' -e '@overkill-todo/' -e 'App\b' .
```

## Daily use

Prerequisites: [mise](https://mise.jdx.dev) and Docker. mise installs everything
else, Rust included (it installs rustup if needed), and `just` recipes always use
mise's tools, so a system Rust or Node on `PATH` doesn't matter.

```bash
mise trust && mise install   # once: just, node, pnpm, Rust, cargo tools
just setup   # once: deps, browsers, backend/.env, database
just dev     # backend :4322 + web :4321 (open this one; it proxies /api)
just         # list every recipe
just ci      # everything CI checks, locally
```

Make yourself an admin after signing up: `just grant-admin you@example.com`.
In dev, verification codes print in the backend log.

| Recipe                                | What it runs                                    |
| ------------------------------------- | ----------------------------------------------- |
| `just test` / `just test-integration` | backend unit / DB-backed tests                  |
| `just test-web`                       | `astro check` + Vitest (real Chromium)          |
| `just e2e`                            | Playwright against the built site, API mocked   |
| `just e2e-integration`                | Playwright signup against `just dev`            |
| `just smoke <url>`                    | read-only checks against a deployed site        |
| `just up`                             | prod-like stack in Docker behind Caddy on :8080 |
| `just bench` / `just bench-db`        | backend criterion benches / full-router + DB    |
| `just bench-web`                      | Vitest benches (helpers, client, AdminPanel)    |

### Benchmarks

Benchmarks catch performance regressions; they're not part of `just ci` because
wall-clock numbers on shared runners are too noisy to gate on. Compare against a
baseline from `main`, on the same machine:

```bash
git switch main && just bench --save-baseline main          # once per main change
git switch my-branch && just bench --baseline main          # prints % change per bench
just bench-web --outputJson .bench/main.json                # frontend, on main
just bench-web --compare .bench/main.json                   # frontend, on your branch
```

- `backend/benches/crypto.rs`: JWT, CSRF, OAuth state, secret cipher, tokens,
  and Argon2 at production cost.
- `backend/benches/domain.rs`: value objects, request DTOs, error fingerprints
  and responses, email templates.
- `backend/benches/middleware.rs`: the real router's middleware stack, no DB.
- `backend/benches/http.rs` (`just bench-db`): `/me`, export, admin list,
  login, and refresh rotation against a throwaway `overkill_todo_bench` database.
- `apps/web/**/*.bench.ts`: Vitest benches in Chromium. Bench mode skips
  `beforeAll`, so set up in the `describe` body; pass `{ throws: true }` to async
  benches so a failure fails the run instead of reporting NaN.

Criterion HTML reports land in `backend/target/criterion/report/index.html`.

## Parallel work (worktrees)

```bash
just wt-new feat/thing        # ../overkill-todo-feat-thing on a new branch off HEAD
just wt-new fix/x origin/main # or off another base; reuses the branch if it exists
just wt-list                  # every checkout and its ports
just wt-rm feat/thing         # drop the checkout + its database (keeps the branch)
```

Each worktree gets its own Postgres (Compose project `overkill-todo-<branch>`),
its own ports (slot N: web `4321+10N`, API `4322+10N`, e2e preview `4323+10N`,
DB `5432+N`, edge `8080+N`) and its own `backend/.env`. All of that lives in its
gitignored `.worktree.env`, which `just` loads, so `just dev`, `just e2e` and
`just db-migrate` there never touch another checkout's server or schema. The main
checkout has no `.worktree.env` and keeps the default ports.

Each worktree has its own `backend/target`, so its first build starts cold.
`sccache` (installed by mise, wired in by the justfile) shares compiled
dependencies between them; `sccache --show-stats` shows the hit rate. Its cache
sits in `~/.cache/sccache`, capped at 10 GiB by default (`SCCACHE_CACHE_SIZE`).
Google OAuth only accepts redirect URIs you've registered, so real Google sign-in
works only on the main checkout's `:4321` (the dev-mode bypass works everywhere).

## Layout

```
backend/            Rust API (see backend/README.md, backend/ARCHITECTURE.md)
apps/web/           Astro + Svelte site, nginx image
packages/           shared design tokens
deploy/compose/     single-host production (Caddy TLS, digests, nightly dumps)
deploy/k8s/         optional kustomize base (CNPG Postgres, Ingress)
scripts/            dev env generation, worktree create/remove
.github/            CI and Dependabot
```

## Conventions

- **Commits:** Conventional Commits, enforced by commitlint (`feat:`, `fix:`,
  `chore:` …). Pre-commit hooks are path-scoped and fast, so don't skip them.
- **Branches:** short-lived branches into `main`; `main` is always deployable.
- **Dependencies:** pnpm refuses releases younger than 24h; Dependabot waits 3
  days; `cargo deny` (advisories, licenses, sources;
  also weekly) and `cargo machete` (unused deps) run in CI.
- **Database:** forward-only migrations, expand then contract; let Postgres
  enforce invariants (constraints, partial unique indexes).
- **Deploys:** an image digest change committed to the deploy config. CI builds
  images; it never holds production credentials.

## Where this came from

Extracted from a production project and trimmed to the auth core. The reasoning
behind most choices lives in the notes vault: start at _Starting a new project_.
