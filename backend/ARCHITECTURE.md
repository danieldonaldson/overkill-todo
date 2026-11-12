# Architecture

## Hexagonal layout

```
src/
├── domain/           # Entities, value objects, domain errors — no dependencies
├── application/      # Use cases and port traits — depends only on domain
│   ├── ports/
│   │   ├── incoming/ # Traits implemented by use cases (called from handlers)
│   │   └── outgoing/ # Traits implemented by adapters (repos, services)
│   └── use_cases/    # Business logic — depends on port traits, not adapters
├── infrastructure/   # All I/O adapters — depends on application + domain
│   ├── api/          # Axum handlers, routes, middleware (HTTP adapter)
│   ├── config/       # Settings loaded from env vars
│   ├── crypto.rs     # `Crypto` handle: Argon2 params, JWT keys, CSRF secret, cipher
│   ├── oauth/        # Google OAuth client
│   ├── persistence/  # Driver-neutral DB traits; postgres/ = sqlx adapter (only sqlx user)
│   ├── services/     # Concrete service impls (argon2, JWT, token gen, …)
│   ├── tasks/        # Background jobs and leader election
│   └── telemetry/    # Tracing, logs, metrics
└── shared/           # Dependency-free utilities (error fingerprinting)
```

Dependencies only point inward: `infrastructure → application → domain`.
The domain imports no `sqlx` or `serde`: Postgres enum mappings live in
`persistence/postgres/models.rs` (`DbUserRole`, `DbOAuthProvider`) and convert with
`From` at the repository boundary.

**`sqlx` is confined to `infrastructure/persistence/postgres/`**, and only
`main.rs` wiring and tests name that module. The `persistence` root is
driver-neutral. The driver is chosen at compile time, so dispatch is static:
`persistence::Database` aliases the selected adapter's handle, and other
infrastructure modules use it only through the root's traits, either as generic
bounds or as trait methods on that alias. The traits are `LeaderLocks` /
`LeaderLock` (leader election in `tasks/leader.rs`), `DatabaseMaintenance`
(`tasks/database_maintenance.rs`) and `DatabaseHealth` (`/health/ready` and the
pool gauges); the root also exports `STATEMENT_LOG_DIRECTIVE` (trace export
filter). Repositories stay `Arc<dyn Port>`, because that is what use cases take and
mock. Each adapter returns a `Persistence` bundle of both. Plumbing failures
surface as the opaque, driver-agnostic `DatabaseError`; repository failures go
through `persist_err` to `DomainError`. There is no `From<sqlx::Error> for
AppError`, so a handler can't `?` a raw driver error. A second driver is a sibling
of `postgres/` implementing the outgoing ports and these traits; selecting it
means pointing the `Database` alias and the `open` call in `main.rs` at it.

`shared/` is a true leaf: it imports nothing from any layer and holds only
`fingerprint`. `Crypto` lives in `infrastructure/crypto.rs` because it is built from
`CryptoSettings` and holds key material. The HTTP-facing `AppError` (with its
`IntoResponse`/`From` mappings) and the helpers that produce it — `csrf`, `jwt`,
`oauth_state` — live in `infrastructure/api` (they import `axum`/`jsonwebtoken`).

Handlers stay thin: they parse the request, run edge admission (Turnstile, device
fingerprint), call one use case, and turn its result into cookies and a response.
Credential checks, password hashing, and minting single-purpose tokens (signup,
reset) happen in use cases. The session access JWT and CSRF token are an HTTP
concern, so `build_session` mints them in `infrastructure/api`, next to the
middleware that verifies them.

DTOs (`application/dto/`) are likewise not a purely inward layer: every DTO derives
`serde` (`Serialize`/`Deserialize`) and `utoipa::ToSchema`, and request DTOs add
`validator::Validate` with `#[validate(...)]` attributes. **Input validation lives
at the DTO boundary** (enforced by the `ValidatedJson` extractor), so the DTO layer
intentionally carries serialization/OpenAPI/validation concerns.

## Routes and the API description

The router and the OpenAPI description are built from the same declarations, so
they can't drift. In `infrastructure/api/routes.rs`, `api_routes()` mounts every
documented route with `utoipa_axum::routes!(handler)`, which takes the method and
path from the handler's `#[utoipa::path]`. The annotation's `path` is relative to
the group it's nested in (`path = "/login"` under `/auth` under `/api/v1`). Use one
`routes!` per path.

- **Protection:** group middleware is applied through the `Guarded` methods
  (`require_auth()`, `require_verified_email()`, `require_fresh_session()`,
  `require_csrf()`). Each applies the `route_layer` _and_ documents the status it
  can return (401/403), so the description can't miss a protection. Call them in the
  order the layers should run, innermost first.
- **State:** the whole tree is `OpenApiRouter<AppState>` (`state.rs`). Handlers keep
  extracting their own `State<Arc<…Deps>>` through `FromRef`. `api_routes()` takes no
  values, so `openapi::openapi()` builds the description without a database.
  Middleware that needs a dependency reads it from a request `Extension` installed
  in `create_router` (as `require_fresh_session` does for the user repository).
- **Errors:** `openapi::finalize()` gives every 4xx/5xx without a body the standard
  `ErrorResponse` body, adds 500 everywhere, and adds 400 wherever a body, query or
  path parameter is taken. Use `ValidatedJson` / `ApiJson` / `ApiQuery` / `ApiPath`,
  never bare `Json` / `Query` / `Path`, so malformed input gets that JSON 400 rather
  than axum's plain-text rejection.
- **Undocumented routes:** `backend/clippy.toml` disallows plain `.route(...)`. A
  route that is deliberately left out needs
  `#[expect(clippy::disallowed_methods, reason = "…")]`. Only the root-level health
  probes (a container-HEALTHCHECK duplicate of `/api/v1/health*`) are exempt.
- **Optional fields:** `Option` request fields need `#[serde(default)]`, or they are
  described as required. `Option` response fields with `skip_serializing_if` need
  `#[schema(nullable = false)]`, because they're omitted, never `null`. A `String`
  with a fixed set of values gets `#[schema(value_type = …)]` with an enum from
  `application/dto/schema_enums.rs`.
- **Committed outputs:** `just api-types` writes `backend/openapi.json` (from
  `examples/export_openapi.rs`) and the web types
  `apps/web/src/lib/api/schema.d.ts`. Commit both; CI fails if either is stale.

## Adding a use case

1. Define the input/output types in `application/dto/` if needed.
2. Add an incoming port trait in `application/ports/incoming/` (one method per operation).
3. Implement the trait in `application/use_cases/`.
4. Wire it in `src/main.rs` (construction) and `infrastructure/api/routes.rs`
   (`.routes(routes!(handler))` in `api_routes()`), then run `just api-types`.
5. Write mock-based unit tests in the use case file and HTTP-level integration tests under `tests/`.

## Adding a repository method

1. Add the method signature to the outgoing port trait in `application/ports/outgoing/`.
2. Implement it in `infrastructure/persistence/postgres/` using `sqlx::query!` macros.
3. Map any `sqlx::Error` with the shared `persist_err` helper (`postgres/error.rs`) —
   `.map_err(persist_err)?` — so a unique violation (`23505`) becomes a
   `DomainError::Conflict` and a missing row a `DomainError::NotFound` instead of an
   opaque `Persistence` string. Only hand-write a `match` when a query needs a
   constraint-specific message.
4. Run `just db-prepare` to update the offline cache.
5. Update any in-memory test stubs that implement the trait.

**Always use `sqlx::query!` (compile-checked macros), never `sqlx::query` (string-based).**

### Row → entity mapping convention

Each table has a `…Row` struct (in `postgres/models.rs`, or local to the repo) that
mirrors the columns; convert it to the domain entity by exactly one of these, in order
of preference:

- **`impl From<XRow> for Entity`** — when the conversion is infallible and needs no
  collaborators (e.g. `OAuthAccountRow`).
- **A private `fn row_to_entity(row) -> Result<Entity, DomainError>`** — when
  reconstruction can fail on its own (re-parsing a value object), so there is nothing to
  inject (e.g. `UserRow` → `User`).
- **A private `fn row_to_entity(crypto, row) -> Result<Entity, DomainError>`** — when the
  mapping needs an injected dependency such as `Crypto` to decrypt an at-rest secret
  (e.g. a row holding an encrypted third-party token). These cannot be `From`/`TryFrom`
  because the trait has no place for the collaborator.

Pick the first form that fits; do not mix a `FromRow`-derive style into new repos.

## Crypto / config

All cryptographic parameters (`ARGON2_*`, `JWT_*`, `CSRF_SECRET`,
`SECRETS_ENCRYPTION_KEY`) are loaded once at startup by `Settings::from_env()`, then
used to build a single `Crypto` handle via `Crypto::from_settings()` in `main.rs`. That
handle is wrapped in an `Arc<Crypto>` and **injected** into the adapters that need it
(JWT/token/password-hasher services, the at-rest secret-cipher repositories, the
OAuth-state helpers) and exposed to the state-free auth/CSRF middleware via an
`Extension<Arc<Crypto>>` layer on the router. Tests build their own handle with
`Crypto::for_testing()` (unit) or the cached `test_crypto()` helper (integration) — there
is no global singleton.

Never read crypto env vars directly in library code — always go through an injected
`Crypto`.

## Telemetry

`infrastructure/telemetry/` holds everything OpenTelemetry-specific. The
application and domain layers only use `tracing` macros.

- **Adapters.** `telemetry::init` picks real or no-op: with
  `OTEL_EXPORTER_OTLP_ENDPOINT` set, spans and metrics go to the collector over
  OTLP/HTTP. Without it (only allowed with `DEV_MODE=true`), a tracer provider
  with no exporter still assigns real trace IDs, so `X-Trace-Id` and log
  correlation behave the same, and metric instruments are no-ops. Logs go to
  stdout (`LOG_FORMAT`: JSON in production) with `trace_id`, and the collector
  ships them.
- **Instrumentation.** Every `#[tracing::instrument]` uses `skip_all` and lists
  the fields it records. `skip(self)` alone records every other argument via
  `Debug`, which is how emails and passwords leaked before. Span names are
  `handler.*`, `usecase.*`, `service.*` and `repo.<table>.<op>` (repositories add
  `fields(db.system = "postgresql")`; sqlx statement events attach to that
  span with the SQL text but never bound parameters). Health endpoints are not
  traced.
- **Redaction.** Never record secrets, tokens, codes or email addresses in a
  span field or log line; log `user_id`, or `email_domain` where the domain
  matters. As a backstop, `redact.rs` scrubs any field whose name looks
  sensitive (`password`, `token`, `code`, `session`, `cookie`, …) and the
  sensitive query parameters in URLs, at both exits: `RedactingSpanProcessor`
  before export and the log formatters on stdout. The collector applies the
  same field-name rule again. Add new sensitive names there, with a test.
- **Metrics.** Instruments in `metrics.rs` are built from an explicit `Meter`
  passed in, so tests can point them at an in-memory exporter. Production code
  takes it from `global_meter()`, which `init` wires up before anything else
  runs. Keep label values bounded (routes use the matched template, unknown
  paths are `"unmatched"`).

## Auth flow overview

- **Registration:** email + password → Argon2id hash → unverified user → email code.
- **Email verification:** 6-digit code (hashed) → mark verified → complete signup → JWT + refresh token.
- **Login:** email + password → Argon2id verify → JWT (RS256, 15 min) + refresh token (SHA-256 hash stored, 30 days) + CSRF token (double-submit cookie).
- **Token refresh:** refresh token → device fingerprint check → atomic rotate (single transaction) → new JWT + new refresh token.
- **OAuth (Google):** authorization-code flow → server-to-server token exchange → Google ID-token signature verified against Google's JWKS (issuer + audience checked) → upsert `oauth_accounts` → auto-link only if email verified by both sides.

## Security invariants (don't regress these)

- Argon2id for all passwords; tunable cost params from config.
- RS256 with algorithm pinning on decode — HS256 is rejected.
- Refresh tokens stored as SHA-256 hashes; raw token never persisted.
- Refresh rotation is atomic (single DB transaction).
- Revoked refresh-token reuse triggers full-family revocation. The reuse/revoked
  check runs _before (regardless of) the expiry check_, so a stolen token replayed
  after its natural expiry still trips session-wide invalidation
  (`refresh_token.rs`).
- Google OAuth ID tokens are JWKS-verified (signature + issuer + audience) before
  any account link/login decision.
- Device fingerprint bound to refresh tokens.
- CSRF: double-submit cookie, HMAC-verified, constant-time comparison.
- `Secure` cookie attribute set in non-dev environments.
- Login brute-force throttling is **handled at the edge, by design** — there is no
  per-account `failed_login`/`locked_until` counter in `login_user.rs`. Password login
  is protected by (a) a reverse-proxy rate limit on `/api/v1/auth/*` (configure one in
  whatever fronts the API) and (b) Cloudflare Turnstile on the login form. High-entropy
  passwords (Argon2id-verified) don't need per-account lockout the way the low-entropy
  6-digit reset/verify codes do — those keep their own atomic attempt cap. If we ever
  drop the edge limiter or Turnstile, revisit this and add an in-app per-account/IP lockout.
- OAuth test bypass (`TEST_OAUTH_BYPASS`) is gated three ways: `#[cfg(debug_assertions)]`
  at compile time, the `TEST_OAUTH_BYPASS` env var at runtime, **and** a startup
  assertion in `main.rs` that refuses to run the bypass unless `DEV_MODE=true`. It is
  therefore absent from a normal release build and cannot run in production.
