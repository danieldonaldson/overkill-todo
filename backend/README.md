# Backend

Rust 2024 / Axum 0.8 auth service in a hexagonal layout, on Postgres via sqlx.
Run everything through the root `justfile`; this file covers what's specific to
the backend.

## Run

```bash
just setup   # once: writes backend/.env from .env.example with fresh dev keys
just dev     # backend on :4322 (auto-reload) + web on :4321, proxying /api
```

`backend/.env.example` is the config **contract**: every variable the server
reads, with comments. `Settings::from_env` reports every missing or malformed
variable at once and the process exits. With `DEV_MODE=true` you also get the
Swagger UI at `http://localhost:4322/docs`.

## Tests

```bash
just test              # unit + property tests, no database (mockall, proptest)
just test-integration  # #[sqlx::test]: each test gets its own fresh database
```

Unit tests mock the outgoing ports, so use cases are tested without I/O.
Integration tests in `tests/` drive the real router against Postgres.

## Queries and the offline cache

Always `sqlx::query!` / `query_as!` (compile-time checked), never the unchecked
forms. After changing any query, regenerate and commit `.sqlx/` so CI and Docker
builds compile without a database:

```bash
just db-prepare        # migrate + regenerate .sqlx/
just db-prepare-check  # what CI runs
```

Migrations live in `migrations/` and are forward-only; see `migrations/README.md`.
They are embedded in the binary and run at startup.

## External services: ports and adapters

Every external dependency sits behind an outgoing port with a real adapter and a
no-op or console adapter. `main.rs` is the only place that chooses, and outside
`DEV_MODE` it **refuses to start** if a no-op would be used:

| Port | Real | Dev fallback |
| --- | --- | --- |
| Email | `SmtpEmailService` | `ConsoleEmailService` (prints to stdout) |
| Bot protection | `CloudflareTurnstileService` | `NoOpTurnstileService` |
| MX validation | `HickoryMxValidator` | `NoOpMxValidator` (only when disabled) |
| Google OAuth | `GoogleOAuthClient` | disabled unless `GOOGLE_OAUTH_ENABLED` |

Adding a provider (payments, storage, LLMs) means a new port in
`application/ports/outgoing/`, an adapter in `infrastructure/`, and a no-op for dev.

## Background work

`infrastructure/tasks/` holds periodic jobs. They run under a Postgres
advisory-lock leader (`leader.rs`), so with N replicas exactly one runs them:

- `account_purger`: anonymizes accounts whose deletion grace period has passed
- `database_maintenance`: deletes expired refresh tokens and one-time codes

## Auth model (what's already built)

Argon2id passwords, RS256 access JWTs (short) plus rotating refresh tokens bound
to a device fingerprint, double-submit CSRF on every cookie-authenticated
route, email verification codes, password reset, Google OAuth (server-side
code flow), Turnstile, MX checks, roles (`user` / `admin` / `super_admin`), account export and scheduled deletion.

See [ARCHITECTURE.md](ARCHITECTURE.md) for layout and conventions.
