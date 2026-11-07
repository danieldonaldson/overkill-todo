# CLAUDE.md

Rust/Axum backend + Astro/Svelte frontend. Read `README.md` and
`backend/ARCHITECTURE.md` before structural changes.

## Commands

- `just dev` runs everything locally; `just ci` is the full gate. Run the
  narrowest check that covers a change (`just test`, `just lint`, `just test-web`)
  and `just ci` before saying a larger change is done.
- After editing any SQL query: `just db-prepare` and commit `backend/.sqlx/`.
- Clippy runs with pedantic + nursery and `-D warnings`. Fix lints; don't add
  `allow`s without a comment saying why.

## Backend rules

- Dependencies point inward only: `infrastructure → application → domain`.
  Domain has no I/O crates.
- New external service = outgoing port + real adapter + no-op adapter, chosen in
  `main.rs`, with production refusing the no-op.
- `sqlx::query!` / `query_as!` only. Migrations are forward-only and must stay
  compatible with the previous release (expand, deploy, then contract).
- New authenticated route groups go above the CSRF `route_layer` in
  `routes.rs`; public signature-verified webhooks go below it.
- Background jobs that must run once go under the leader lock in `main.rs`.
- Errors: domain errors map to `AppError` in `infrastructure/api`; never leak
  internal error text to clients outside `DEV_MODE`.

## Frontend rules

- Pages are static Astro; interactivity is Svelte 5 islands (runes).
- Runtime config comes from `window.__PUBLIC_CONFIG` via `public/config.js`,
  rendered from `PUBLIC_*` env at container start. No secrets in the frontend.
- Inline scripts are CSP-pinned by sha256 in `nginx.conf`; changing one means
  updating the hash (the CSP spec fails otherwise).
- Component tests: Vitest browser mode. Flows: Playwright with the API mocked
  (`tests/`); real-backend flows go in `tests/integration/`.

## Don't

- Commit `.env`, `backend/.env`, decrypted secrets, or `deploy/compose/backend.env`.
- Set `DEV_MODE=true` anywhere but local.
