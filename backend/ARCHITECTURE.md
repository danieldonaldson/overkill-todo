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
│   ├── oauth/        # Google OAuth client
│   ├── persistence/  # sqlx Postgres repositories
│   └── services/     # Concrete service impls (argon2, JWT, token gen, …)
└── shared/           # Dependency-light cross-cutting utilities (crypto, fingerprint)
```

Dependencies only point inward: `infrastructure → application → domain`.
`shared/` is used by all layers and is a true leaf: it carries no layer-specific
imports. The HTTP-facing `AppError` (with its `IntoResponse`/`From` mappings) and the
helpers that produce it — `csrf`, `jwt`, `oauth_state` — live in `infrastructure/api`
(they import `axum`/`sqlx`/`jsonwebtoken`), so `shared/` holds only `crypto` and
`fingerprint`.

DTOs (`application/dto/`) are likewise not a purely inward layer: every DTO derives
`serde` (`Serialize`/`Deserialize`) and `utoipa::ToSchema`, and request DTOs add
`validator::Validate` with `#[validate(...)]` attributes. **Input validation lives
at the DTO boundary** (enforced by the `ValidatedJson` extractor), so the DTO layer
intentionally carries serialization/OpenAPI/validation concerns.

## Adding a use case

1. Define the input/output types in `application/dto/` if needed.
2. Add an incoming port trait in `application/ports/incoming/` (one method per operation).
3. Implement the trait in `application/use_cases/`.
4. Wire it in `src/main.rs` (construction) and `infrastructure/api/routes.rs` (routing).
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
  check runs *before (regardless of) the expiry check*, so a stolen token replayed
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
