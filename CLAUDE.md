# CLAUDE.md

Rust/Axum backend + Astro/Svelte frontend. Read `README.md` and
`backend/ARCHITECTURE.md` before structural changes.

## Commands

- `just dev` runs everything locally; `just ci` is the full gate. Run the
  narrowest check that covers a change (`just test`, `just lint`, `just test-web`)
  and `just ci` before saying a larger change is done.
- After editing any SQL query: `just db-prepare` and commit `backend/.sqlx/`.
- After changing a DTO, handler annotation or route: `just api-types` and commit
  `backend/openapi.json` and `apps/web/src/lib/api/schema.d.ts`. Never edit
  either by hand.
- Clippy runs with pedantic + nursery and `-D warnings`. Fix lints; an `allow` or
  `expect` must give `reason = "…"` (enforced).
- Parallel checkouts: `just wt-new <branch>` (own DB/ports), never bare
  `git worktree add`. `just wt-list` / `just wt-rm <branch>`.

## Backend rules

- Dependencies point inward only: `infrastructure → application → domain`.
  Domain has no I/O crates.
- New external service = outgoing port + real adapter + no-op adapter, chosen in
  `main.rs`, with production refusing the no-op.
- `sqlx::query!` / `query_as!` only. Migrations are forward-only and must stay
  compatible with the previous release (expand, deploy, then contract).
- Mount routes with `.routes(routes!(handler))` in `api_routes()` (`routes.rs`),
  so they're in the OpenAPI description; plain `.route(...)` fails lint. Protect
  groups with the `Guarded` methods (`require_auth()` etc.), not bare
  `route_layer`s. New authenticated groups go above `.require_csrf()`; public
  signature-verified webhooks go below it.
- Extract input with `ValidatedJson` / `ApiJson` / `ApiQuery` / `ApiPath`, not
  bare axum `Json` / `Query` / `Path`, so malformed input gets the JSON 400.
- Background jobs that must run once go under the leader lock in `main.rs`.
- Errors: domain errors map to `AppError` in `infrastructure/api`; never leak
  internal error text to clients outside `DEV_MODE`.
- Telemetry: never record secrets, tokens, codes or emails in spans or logs;
  `#[tracing::instrument]` always uses `skip_all` with explicit fields.

## Frontend rules

- Pages are static Astro; interactivity is Svelte 5 islands (runes).
- API payload types in `src/lib/types.ts` are aliases of the generated
  `src/lib/api/schema.d.ts`. Don't hand-write payload shapes; change the backend
  DTO and run `just api-types`.
- Runtime config comes from `window.__PUBLIC_CONFIG` via `public/config.js`,
  rendered from `PUBLIC_*` env at container start. No secrets in the frontend.
- Inline scripts are CSP-pinned by sha256 in `apps/web/Caddyfile`; changing one means
  updating the hash (the CSP spec fails otherwise).
- Hydration: primary page content is `client:load`; content that starts below the
  fold is `client:visible`. The first `client:visible` adds an inline hydration
  script, so add its sha256 to the Caddyfile.
- The dashboard is optimistic: render first, never wait on a sign-in check.
  The server authorises every call and `HttpClient` handles 401/403; panels
  handle their own failures. Static HTML holds no user data, and privileged UI
  (e.g. the Admin link) is shown only once the role is known, never guessed.
- The signed-in user lives in `lib/auth/session.svelte.ts`, shared by all
  islands. The dashboard header is persisted (`transition:persist`), so it
  must hold no page-specific state.
- Component tests: Vitest browser mode. Flows: Playwright with the API mocked
  (`tests/`); real-backend flows go in `tests/integration/`.

## Secrets

- SOPS + age under `deploy/secrets/<env>/`; who reads what is
  `deploy/secrets/access.yaml`. `.sops.yaml` is generated: change the policy,
  then `just secrets-sync`. Create secrets with `just secrets-new` (from stdin),
  never as a plaintext file there.

## Don't

- Commit `.env`, `backend/.env`, decrypted secrets, or `deploy/compose/.secrets/`.
- Set `DEV_MODE=true` anywhere but local.
