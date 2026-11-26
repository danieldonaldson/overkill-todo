# Architecture

## Where does X go?

| Adding…                                   | Goes in                                                                                           |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------- |
| Business rule, entity, value object       | `crates/domain/`                                                                                  |
| Use case                                  | `crates/application/src/use_cases/` + incoming port (see [Adding a use case](#adding-a-use-case)) |
| Request/response shape                    | `crates/application/src/dto/`, then `just api-types`                                              |
| Endpoint, middleware, extractor, cookie   | `crates/api/src/` (`handlers/`, `middleware/`, `extractors/`), mounted in `routes.rs`             |
| SQL query or repository                   | outgoing port in `application` + `crates/persistence-postgres/src/`, then `just db-prepare`       |
| Migration                                 | `backend/migrations/` (forward-only, expand/contract)                                             |
| External service (email, captcha, OAuth…) | outgoing port in `application` + real and no-op adapters in `crates/services/src/`                |
| Setting / env var                         | `crates/platform/src/config/settings.rs`                                                          |
| Token kind or token claim rule            | `crates/platform/src/crypto/token.rs` (never build a `Validation` in an adapter)                  |
| Key material, hashing primitive           | `crates/platform/src/crypto/`                                                                     |
| Metric, span or log plumbing              | `crates/platform/src/telemetry/`                                                                  |
| Background or leader-only job             | `crates/server/src/infrastructure/tasks/`, started in `main.rs`                                   |
| Wiring (which adapter is used)            | `crates/server/src/main.rs`                                                                       |
| DB setup in a server test or bench        | a fixture in `crates/persistence-postgres/src/testing.rs`, never raw SQL                          |
| Full-stack HTTP test                      | `crates/server/tests/`                                                                            |

If the code needs a crate the target doesn't depend on, it's in the wrong
place. Move it, don't add the edge: `crates/server/tests/crate_graph.rs` will
refuse the edge anyway.

### Workspace gotchas

- **`RUST_LOG` uses crate names.** Each crate logs under its own name
  (`api`, `services`, `persistence_postgres`…), and `main.rs` under
  `overkill_todo`. `RUST_LOG=overkill_todo=debug` now covers only startup and
  shutdown; use e.g. `api=debug,application=debug`. The default filter is
  built from `server::APP_TARGETS`, and a new crate must be added there (the
  gate checks).
- **Test helpers sit behind `test-support`.** `UserBuilder` (`domain`),
  `LogCapture` (`application`), the keyring/crypto and metrics test helpers
  (`platform`) and the DB fixtures (`persistence-postgres`) are compiled only
  under `#[cfg(any(test, feature = "test-support"))]`. If a helper is "not
  found" in another crate's tests, add
  `<crate> = { path = "../<crate>", features = ["test-support"] }` to that
  crate's `[dev-dependencies]`.
- **New third-party dependency in `domain` or `application`** fails the gate
  until it's added to that crate's allowlist in `crate_graph.rs`. That's
  deliberate. Ask whether it belongs in an adapter first.
- **sqlx config is split.** `sqlx.toml` lives in `crates/persistence-postgres/`,
  but the offline cache is `backend/.sqlx/` at the workspace root
  (`just db-prepare` writes it).
- **Unused fixtures fail the gate.** Every `pub fn` in
  `persistence-postgres/src/testing.rs` must be called from another crate's
  tests or benches.
- **The release build disables default features** (`-p server
--no-default-features`, which drops Swagger UI). CI runs clippy that way too,
  so code under `#[cfg(not(feature = "swagger-ui"))]` is linted.

## Hexagonal layout

```
crates/
├── domain/src/               # Entities, value objects, domain errors — no I/O dependencies
├── application/src/          # Use cases, DTOs and port traits — depends only on domain + shared
│   ├── dto/                  # Request/response DTOs (serde, utoipa, validator)
│   ├── ports/
│   │   ├── incoming/         # Traits implemented by use cases (called from handlers)
│   │   └── outgoing/         # Traits implemented by adapters (repos, services)
│   └── use_cases/            # Business logic — depends on port traits, not adapters
├── shared/src/               # Dependency-free leaf (error fingerprinting, redaction rules)
├── platform/                 # Pure infrastructure (config, crypto/token rules, TLS, telemetry, persistence plumbing traits) — depends only on shared
├── persistence-postgres/     # sqlx Postgres adapter (only sqlx normal-dependency user)
├── services/                 # External service, Google OAuth & browser-report sink adapters
├── api/                      # HTTP adapter (handlers, middleware, extractors, router, OpenAPI)
│   └── examples/             # export_openapi
└── server/                   # Composition root, background tasks, binary, tests, benches
    ├── src/
    │   ├── main.rs
    │   └── infrastructure/
    │       └── tasks/        # Background jobs and leader election
    ├── tests/                # Full-stack integration tests + crate_graph gate
    └── benches/              # Criterion benchmarks
```

Dependencies only point inward: `server → {persistence-postgres, services, api} → {application, platform}`,
with `application → {domain, shared}` and `platform → {shared}` (`domain` has no workspace dependencies).
Adapters never depend on each other, and `platform` is pure infrastructure below the adapters rather than depending on `application`. The crate-graph
gate (`crates/server/tests/crate_graph.rs`) enforces this at the workspace level
and locks `domain` and `application` third-party dependencies to an allowlist.
The domain imports no `sqlx` or `serde`: Postgres enum mappings live in
`crates/persistence-postgres/src/models.rs` (`DbUserRole`, `DbOAuthProvider`) and convert with
`From` at the repository boundary.

**`sqlx` is confined to `crates/persistence-postgres/`**, and only
`main.rs` wiring, tests, and benches name that crate (`tests/` and `benches/`
also allow `#[sqlx::test]` and `PgPool`/`PgPoolOptions`). **Architectural decision — driver-neutral plumbing vs. repository ports:**
plumbing interfaces with no business meaning live in `platform::persistence`, whereas
all business repository ports remain in `application::ports::outgoing`. The driver is chosen at compile
time, so dispatch is static: `server::Database` aliases `persistence_postgres::Database`,
and other modules use `platform::persistence` traits, either as
generic bounds or as trait methods on that alias. The traits are `LeaderLocks` /
`LeaderLock` (leader election in `tasks/leader.rs`), `DatabaseMaintenance`
(`tasks/database_maintenance.rs`) and `DatabaseHealth` (`/health/ready` and the
pool gauges); `persistence-postgres` also exports `STATEMENT_LOG_DIRECTIVE` (trace export
filter). Repositories stay `Arc<dyn Port>`, because that is what use cases take and
mock. `persistence-postgres` returns a `Persistence` bundle of both. Plumbing failures
surface as the opaque, driver-agnostic `DatabaseError`; repository failures go
through `persist_err` to `DomainError`. There is no `From<sqlx::Error> for
AppError`, so a handler can't `?` a raw driver error. A second driver is a sibling
crate of `persistence-postgres` implementing the outgoing ports and these traits; selecting it
means pointing the `server::Database` alias and the `open` call in `main.rs` at it.

`shared/` is a true leaf: it imports nothing from any layer and holds only
`fingerprint` and `redact` (the field-name and URL-query rules, shared by the
telemetry exits and the browser-report use case). `Crypto` and shared JWT envelope
validation (`platform::crypto::token`) live in `crates/platform/src/crypto/` because `Crypto` is built from
`CryptoSettings`, holds key material, and guarantees identical `typ`/`iss`/`aud`/`iat`/`exp`
rules across all four token kinds (`Session`, `Signup`, `Reset`, `OAuthState`). The HTTP-facing `AppError` (with its
`IntoResponse`/`From` mappings) and the helpers that produce it — `csrf`, `jwt`,
`oauth_state` — live in `crates/api` (they import `axum`/`jsonwebtoken`, delegating token signing and verification to `platform::crypto::token`).

Handlers stay thin: they parse the request, run edge admission (Turnstile, device
fingerprint), call one use case, and turn its result into cookies and a response.
Credential checks, password hashing, and minting single-purpose tokens (signup,
reset) happen in use cases. The session access JWT and CSRF token are an HTTP
concern, so `build_session` mints them in `crates/api`, next to the
middleware that verifies them.

**Architectural decision — OpenAPI and validation metadata on `application` DTOs:**
DTOs (`crates/application/src/dto/`) derive `serde` (`Serialize`/`Deserialize`) and
`utoipa::ToSchema` on purpose, and request DTOs add `validator::Validate` with
`#[validate(...)]` attributes. Keeping one shared contract between the use cases,
the OpenAPI description (`crates/api`), and the `ValidatedJson` extractor avoids
duplicating every request/response struct across `application` and `api`; `crate_graph.rs`
explicitly allowlists these schema/validation dependencies in `application`.

## Routes and the API description

The router and the OpenAPI description are built from the same declarations, so
they can't drift. In `crates/api/src/routes.rs`, `api_routes()` mounts every
documented route with `utoipa_axum::routes!(handler)`, which takes the method and
path from the handler's `#[utoipa::path]`. The annotation's `path` is relative to
the group it's nested in (`path = "/login"` under `/auth` under `/api/v1`). Use one
`routes!` per path.

- **Protection:** group middleware is applied through the `Guarded` methods
  (`require_auth()`, `require_verified_email()`, `require_fresh_session()`,
  `require_admin()`, `require_super_admin()`, `require_csrf()`). Each applies the
  `route_layer` _and_ documents the status it can return (401/403), so the
  description can't miss a protection. Call them in the order the layers should
  run, innermost first. Admin roles are enforced on the group, not only by the
  handler's `AdminUser` / `SuperAdminUser` extractor.
- **Public routes:** `tests/route_guards_integration.rs` walks every operation in
  the description. Each one must be in its `PUBLIC_ROUTES` list or refuse a
  request without a session, and every `/admin` operation must refuse a Member.
  Admin-only routes go under `/api/v1/admin` and through `admin_guards()` /
  `super_admin_guards()`, which check the role after the fresh-session check (so
  a just-promoted user gets a 401 and refreshes). A
  new public route goes on that list, with a reason, and gets a property in the
  threat model.
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
- **Security:** the description's security requirements come from the guards
  too. `require_auth()` adds `session_cookie` and `require_csrf()` adds
  `csrf_header` wherever it adds its 403. Only `/auth/refresh` names
  `refresh_cookie`, and OAuth start names the session as optional (link mode),
  both by hand. The session cookie is the only credential: an `Authorization`
  header is ignored. `openapi.rs` tests fail if a route names an undefined
  scheme, or if the CSRF requirement differs from where `csrf_middleware`
  checks it. `PUBLIC_OPERATIONS` there lists the routes that need no session.
- **Constraints:** a request field's `#[validate(...)]` rule is mirrored in
  `#[schema(...)]` (`min_length`/`max_length`, `format = Email`, `pattern`), so
  the fuzzer's "valid input is accepted" check holds. A rule that depends on
  server state can't be described. The operation then skips that check, with
  its reason, in `apifuzz/schemathesis.toml`.
- **Fuzzed:** every described route is fuzzed against the description in
  `just ci` (`just fuzz-api`, README "API fuzzing"). A new route is covered
  automatically and must keep the run clean. A wrong method gets the JSON 405
  from `error::method_not_allowed`.
- **Committed outputs:** `just api-types` writes `backend/openapi.json` (from
  `crates/api/examples/export_openapi.rs`) and the web types
  `apps/web/src/lib/api/schema.d.ts`. Commit both; CI fails if either is stale.

### Dev-mode differences a client can see

The fuzzer runs the backend with `DEV_MODE=true` (console email, no-op Turnstile
and MX checks, the fixed code `123456` for `test-*` addresses). Only these
responses differ from production:

- 5xx messages carry the internal error text (`error.rs`). Any 5xx already
  fails a fuzz run.
- Cookies are set the same way. The `__Host-` cookies are `Secure` in both
  modes, and plain-HTTP clients such as the fuzzer simply send them back.
- The Swagger UI is served at `/docs` (only in builds with the `swagger-ui`
  feature).

4xx bodies are identical in both modes. A new `DEV_MODE` branch in API
behaviour belongs on this list, because the fuzzer can't see past it.

## Adding a use case

1. Define the input/output types in `crates/application/src/dto/` if needed.
2. Add an incoming port trait in `crates/application/src/ports/incoming/` (one method per operation).
3. Implement the trait in `crates/application/src/use_cases/`.
4. Wire it in `crates/server/src/main.rs` (construction) and `crates/api/src/routes.rs`
   (`.routes(routes!(handler))` in `api_routes()`), then run `just api-types`.
5. Write mock-based unit tests in the use case file and HTTP-level integration tests under `crates/server/tests/`.

## Adding a repository method

1. Add the method signature to the outgoing port trait in `crates/application/src/ports/outgoing/`.
2. Implement it in `crates/persistence-postgres/src/` using `sqlx::query!` macros.
3. Map any `sqlx::Error` with the shared `persist_err` helper (`persistence-postgres/src/error.rs`) —
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

`crates/platform/src/telemetry/` holds everything OpenTelemetry-specific (`services/src/client_reports.rs` adapts browser reports onto it). The
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
- **Auth metrics and events (OT-007).** `AuthMetrics` counts credential and
  code checks, lockouts, bot-check rejections and session anomalies. The
  handlers record them from the use cases' typed errors (`LoginError`,
  `VerifyEmailError`, `ResetCodeError`, `VerifyMagicLinkError`,
  `RefreshError`), so no response changes. Every label is a closed enum with
  `as_str()`, never request data. To name the account, the use cases write
  `tracing` events with an `event` field (`auth.code.locked`,
  `auth.code.refused_locked`, `auth.sign_in.failed`), `user_id` and `flow`. A
  new auth outcome gets a label variant and, if it names an account, an event.
  The no-personal-data test in `tests/auth_metrics_integration.rs` covers both.
  What's measured and alerted is in `deploy/compose/README.md` (Auth security).
- **Browser reports (OT-006).** `POST /api/v1/client-telemetry` takes
  `ClientReportBatch` (closed enums, capped text, no user ID field) through
  the `RecordClientTelemetry` port. The use case maps the page onto
  `PAGE_TEMPLATES` and a boundary error's island onto `ISLAND_TEMPLATES`
  (shared with the web guard test via `tests/fixtures/client_islands.json`),
  each with `other`, caps the `release` label at 5 values per replica (its own
  `APP_VERSION` reserved), re-scrubs every text field with the rules in
  `use_cases/telemetry/scrub.rs` (the browser's copy shares
  `tests/fixtures/client_scrub_cases.json`), fingerprints errors, and keeps a
  per-replica budget (`CLIENT_TELEMETRY_MAX_EVENTS_PER_SEC`). The
  `ClientReportSink` adapter in `crates/services/src/client_reports.rs` logs
  `client_error` events and counts errors from the site's own scripts
  (`browser.error`), page loads (`browser.page_load`, `authenticated` from the
  session) and dropped events, and records Web Vitals, navigations and island
  timings in histograms. Navigation labels are made consistent in the use case
  rather than rejected (a `hard` load is always `view_transition=none`, an
  aborted one has no durations), so every request the schema allows is
  accepted, as the fuzzer checks. Every metric label comes from an enum or an
  allowlist. The route is public, outside
  CSRF, untraced (`UNTRACED_ROUTES`) but still in request metrics, capped at
  16 KB (413), and attributes a report only from the session.

## Audit log

`audit_events` (OT-008) is a permanent record of privileged and
security-relevant account actions: role changed (in the app or by
`just grant-admin`), password changed, set or reset, sign-in method linked or
unlinked, deletion requested, cancelled or carried out by the purge, data
exported, and all sessions signed out after a reused session token.

- **Event if and only if the action took effect.** A use case builds the
  domain `AuditEvent` and passes it to the repository method that makes the
  action's primary write (`save_role_change`, `save_audited`,
  `update_password`, the OAuth link save/delete, `anonymize_account`,
  `revoke_all_for_user_audited`). That method inserts it with
  `insert_audit_event` in its own transaction. A refusal, conflict or "nothing
  to do" (for example unlinking a provider that isn't linked) returns before
  the insert, and a failed insert rolls the action back and surfaces as a 5xx.
  Export writes nothing else, so it uses `AuditLogRepository::record` before
  returning the data.
- **Adding a kind.** Add the `AuditKind` variant, widen
  `audit_events_kind_check` in a new migration (`NOT VALID`, then validate),
  and name the `#[sqlx::test]`s proving its atomicity in `atomicity_tests` in
  `persistence-postgres/tests/audit_log_integration.rs`. There's no wildcard arm, so a kind without
  tests doesn't compile.
- **Minimal personal data.** Events identify people by account ID only. Kind,
  actor and details are closed types (old and new role, OAuth provider), so
  nothing free-form can be stored. Readers show the account's current email,
  or "Deleted account" once it's anonymised.
- **Append-only, 7 years.** Triggers refuse every `UPDATE`, every `TRUNCATE`
  and any `DELETE` of an event younger than 7 years, even though the app owns
  the table (a `REVOKE` wouldn't hold against the owner). The hourly
  `database_maintenance` pass deletes events past 7 years. The guard doesn't
  stop the table owner or a superuser from dropping the trigger or restoring a
  backup. That would show up as a migration in review; a separate, less
  privileged runtime role is the later hardening step.
- **Reading.** Super Admins: `GET /api/v1/admin/audit-events`, filterable by
  account (affected or acting) and kind. Pages are chosen by ID before the
  joins. A Member: `GET /api/v1/users/me/security-activity`, their own events
  only, with the actor reduced on the server to `you`, `system` or
  `administrator`.

## Lists and todos

Decisions and their reasons: `openspec/changes/ot-013-todos/design.md`.

- **Ownership is in the lookup.** Every list query takes the caller's ID, so
  another Member's list answers exactly like one that doesn't exist (P-35).
- **One lock per write.** A write locks its list first, or the `users` row when
  creating a list, so limits, positions and versions are decided in order.
- **Versions.** A change names the version the Member saw; a stale one answers
  409 with the current row. The same state already stored succeeds unchanged.
- **Deletion is versioned too.** Deleting a list also names its todo count, so
  a todo added after the confirmation asks again with the current name and
  count. Deleting a todo or list that is already gone is success to the client.
- **Creation keys** make a repeated add or create return the first row.
- **Erasure.** Purging an account deletes its lists in the same transaction, and
  each purge run sweeps lists an older release left behind.
- **The client** (`apps/web/src/lib/lists/`) is confirmed state plus a queue of
  unsaved operations, one save at a time. Renames and todo deletions are
  optimistic; a list deletion waits for the server. Read design D7 before
  changing it.

## Auth flow overview

- **Registration:** email + password → Argon2id hash → unverified user → email code.
- **Email verification:** 6-digit code (hashed) → mark verified → complete signup → JWT + refresh token.
- **Login:** email + password → Argon2id verify → JWT (HS512, 15 min) + refresh token (SHA-256 hash stored, 30 days) + CSRF token (double-submit cookie).
- **Token refresh:** refresh token → device fingerprint check → atomic rotate (single transaction) → new JWT + new refresh token.
- **OAuth (Google):** authorization-code flow → server-to-server token exchange → Google ID-token signature verified against Google's JWKS (issuer + audience checked) → upsert `oauth_accounts` → auto-link only if email verified by both sides.

## Security invariants (don't regress these)

The full list of what auth must guarantee, against which attackers, and which
test, proof or gate covers each property (or why it isn't covered yet) is the
threat model: [`docs/security/threat-model.md`](../docs/security/threat-model.md).
`just threat-model-check` (in `just ci`) fails if a property has no check or
names one that no longer exists. When you add or change an auth behaviour, add
or update its property there; the end of that file explains how. The list
below is the short version.

### Proofs (`just verify`)

Some properties are proven with [Kani](https://model-checking.github.io/kani/),
a bounded model checker, for every input within stated bounds rather than for
sampled ones. The harnesses sit in a `#[cfg(kani)] mod verification` next to
the code they prove, and each has a doc comment stating its bounds.
`just verify` runs them all (31 harnesses, 2 min 15 s warm on 2026-11-26, most
of it the role-name and email proofs; the list proofs take under 15 s, the
validation kernel 10 s of that). It's in `just ci`, and CI runs it as its own
job.

- **Setup:** `mise install` installs the pinned `kani-verifier` (`.mise.toml`).
  Run `cargo kani setup` once to fetch Kani's own nightly toolchain, which
  doesn't touch the pinned Rust. CI reads the same pin.
- **What's proven:**
  - the code-entry lockout decision: the exact window schedule, never
    reissued while locked or in the cooldown, reissued once both waits pass, no
    negative waits (P-10, P-13)
  - one-time codes and magic links accepted only before their expiry and
    under the attempt cap (P-19)
  - a refresh token's status, where revoked wins over expired, and revoking
    keeps the first time (P-15)
  - role changes: no escalation for any actor, current and new role, no
    Super Admin removing their own role, and only exact role names read
    back (P-08)
  - the deletion lifecycle: deleted is terminal, rescheduling keeps the date,
    no purge before the date, and the two columns round-trip (P-28)
  - email normalisation for short ASCII: case and surrounding whitespace
    don't change the identity, and normalising twice changes nothing (P-30)
  - todo order (`todo_order.rs`, OT-013 D6): a move lands strictly between its
    neighbours for every pair of `i64` positions, or asks for renumbering
    exactly when no integer fits; renumbering keeps the order and puts the
    moved todo in its slot (5 positions; a proptest covers 1,000); a move takes
    the slot its neighbours name (3 todos, 2 IDs each way)
  - appending (`todo.rs`): one gap below the last open todo, strictly
    increasing for every `i64` unless within a gap of `i64::MAX`. That is out of
    reach, also proved: no write raises a list's highest position by more than
    one gap and renumbering stays under 65,537 gaps, so it takes about 2^43
    writes to one list (and the unique constraint would refuse it)
  - a todo is open or done, never both or neither, over the two columns'
    presence (`todo_state`, which `Todo::from_persistence` uses)
  - names and titles (`single_line.rs`): trimmed as `str::trim` trims, counted
    in code points, refused when blank, too long or holding a control
    character, U+2028 or U+2029. Bound: 6 symbolic `char`s with `max <= 4`,
    since CBMC can't unwind hundreds of characters; a proptest ties the
    allocation-free kernel to `str::trim` at 100 and 500

  The proofs cover the domain's decisions, not the SQL around them. List
  versions are bumped in SQL, so the version rules are left to the server
  model (OT-013 task 7), not Kani. The
  concurrent writes are P-11, P-12 and P-14, modelled below; the
  last-Super-Admin guard and the purge job's due check are SQL, and tested.

- **Time is an argument.** Policy that depends on the clock takes `now`, and
  callers pass `Timestamp::now()`, so a proof can cover every time.
- **Prove integers, not jiff.** Kani can't get through jiff's timestamp
  arithmetic in reasonable time. The lockout policy is written over integer
  microseconds (`reissue_at` and friends in `attempt_lockout.rs`), and the
  `Timestamp` methods are thin conversions onto it. Follow the same pattern for
  new proofs.
- **Make a proof fail before trusting it:** break the rule it checks on a
  scratch edit, and see `just verify` fail with a counterexample.
- **Strings become code points.** CBMC can't follow `String` allocation or
  long strings, so a text rule is a kernel over `&[char]` (as
  `check_single_line`), proved for a few symbolic `char`s, with a proptest
  tying it to the real function at real lengths.

### Formal coverage of lists (`just formal-coverage`)

[`docs/formal-coverage.yaml`](../docs/formal-coverage.yaml) names, for every
scenario in OT-013's specs and for P-35, the proof, model, replay or test that
covers it. A requirement with no proof or model says why none applies. The
gate (`scripts/formal-coverage.sh`, in `just ci` and an always-run CI job, under
a second) fails when a scenario has no entry or an entry names one the specs no
longer have, or when a named check is gone or wouldn't run: a Kani harness
without `#[kani::proof]` or stated bounds, a test ignored directly or through
`cfg_attr` (except the replays `just models` runs), a skipped or conditionally
skipped web test, a flow under `apps/web/tests/integration` (which `just ci`
doesn't run), or a Quint invariant `scripts/models.sh` doesn't check.
`pending: <task>` is allowed while that task is unticked, also beside checks
when a test covers a scenario now and a model is still owed;
`just formal-coverage --final` allows none. A scenario no automated check can
reach (a load test needing the capped stack) is an `exception:` with its
reason, which `--final` accepts. It reads the change's
spec deltas while the change is active and `openspec/specs/` once archived.
Fixture tests: `scripts/formal-coverage.test.sh`, run first by the recipe.

### Concurrency models (`just models`)

Races between requests are modelled with [Quint](https://quint-lang.org)
in `models/`, and every model is tied to the code by replaying its traces
against the real repositories.

- **What's modelled:** `attempt_claim.qnt` is one user's one-time code raced
  by 3 guesses, 2 code-issuing requests (with a possibly failed email), the
  expiry cleanup and time passing (modules `email_codes` and `reset_codes`). `refresh_rotation.qnt` is one
  Member's refresh tokens raced by 3 refreshes, a sign-out and the cleanup.
  Each request is the sequence of SQL statements it really runs, one model step
  per statement (or per transaction). The invariants are threat model P-11,
  P-12, P-14 and P-15, each named in its property's `Checks:`.
- **What it checks:** Apalache (`quint verify`) checks every interleaving up
  to 8 steps from any starting code; `quint run` searches 10 000 random
  executions of 30 steps. The `unguarded_upsert`, `claim_by_user` and `lock_only_guard` modules
  model the code before OT-010 slice 4 and must still have counterexamples,
  which keeps the invariants from becoming vacuous.
- **Conformance:** `quint run --out-itf` exports 300 random traces per model,
  plus witness traces that reach a refused reissue, and
  `tests/model_conformance.rs` replays each step through the repository method
  that issues that statement, one connection per request, checking results and
  stored rows after every step. A change to a repository's SQL that the model
  doesn't allow fails there, naming the trace and step. The suite is
  `#[ignore]`d in `just test-integration`, because it needs the exported
  traces.
- **Not covered:** inside one statement, the model assumes atomicity; P-12
  records the one place a snapshot read can still give a misleading message.
  Logins and other users aren't modelled.
- **Setup:** `mise install` installs the pinned Quint and the Java that
  Apalache needs (`.mise.toml`); Quint downloads its Apalache release on first
  use. `just models` takes about 4 minutes and is in `just ci`; CI runs it as
  its own job with the same pins (`scripts/models.sh`).
- **When you change a statement** the models describe (the attempt claim, the
  issuing upsert, rotation, revocation, cleanup), change the model step too,
  then see `just models` fail on a scratch break of the new rule before
  trusting it.

### Protocol model (`just protocols`)

Password sign-in, refresh with rotation and reuse detection, Google sign-in and
the CSRF double-submit are modelled with [Tamarin](https://tamarin-prover.com)
in `models/auth_protocols.spthy`, and proven against an active network attacker.

- **Attacker:** Dolev-Yao. They read, replay and forge anything outside TLS,
  hold their own account and Google identity, run their own client, make a
  Member's browser open any URL or submit any cross-site form, and (threat
  model A3) can steal a browser's cookies, its device fingerprint or a
  password. Each lemma says which thefts it allows.
- **Assumptions:** TLS is a secure channel; the MACs (HMAC-SHA-512 for the
  session JWT and OAuth state, HMAC-SHA-256 for the CSRF token) are perfect
  under keys outside the database; Google is honest; cookies are browser state
  bound to the app's host (the `__Host-` prefix makes browsers enforce it); the
  browser follows `SameSite=Lax` and sends no
  non-simple cross-site body without a preflight (the CSRF lemma drops that and
  attaches every cookie). No bound on sessions, browsers, accounts or messages.
  Time, logout and concurrency are outside it (Kani and Quint cover those).
- **What's proven:** the lemmas named in threat model P-15, P-16, P-20, P-21,
  P-22 and P-33, plus sanity lemmas that each flow can complete.
- **Broken variants:** each check the code makes has an `#ifdef` flag that
  removes it (`NO_STATE_CHECK`, `NO_CSRF_CHECK`, `NO_CSRF_BINDING`,
  `NO_FINGERPRINT`, `NO_REVOKE_ALL`, `NO_REVOKED_CHECK`, `ACCEPT_SIMPLE_BODIES`,
  `INSECURE_CHANNEL`), and `scripts/protocols.sh` requires the lemma that relies
  on it to be attacked, with the helper lemmas still proven in that run.
  `NO_HOST_PREFIX` is the code before the cookies got the `__Host-` prefix,
  where an attacker can write the app's cookies: login CSRF (through the
  `oauth_state` or the session cookies) must still be attacked there, and the
  CSRF user binding must still hold.
- **What's proven by construction:** two refresh lemmas restate the
  restrictions that encode the SQL (rotate once, revoke-all), so they can't
  catch a change to that SQL; the integration tests their rules name and the
  Quint replay do.
- **Conformance:** a symbolic model can't replay against the code, so each
  rule that encodes a check the code must make names, in a `// Checks:` comment
  right above it, the integration test that shows the code makes it.
  `just threat-model-check` fails if a named test no longer exists.
- **Setup:** `mise install` installs the pinned Tamarin and Maude (official
  GitHub releases, through mise's `github:` backend). `just protocols` takes
  about a minute and is in `just ci`; CI runs it as its own job when the model
  or the script changes (its result depends on nothing else; the tests its
  rules name are checked by the always-run threat-model job).
- **Search hints:** the model keeps server state in persistent facts with
  restrictions (a token rotates once; revoke-all revokes every token that
  exists at that moment), gives the attacker's connections their own fresh
  ids, and sends every session through one `Server_issue` rule. Each of these
  is what makes the proofs terminate; check the run time after changing them.
- **When you change a check** a rule names (the CSRF middleware, the OAuth
  callback's state check, refresh), change the rule too, and see its variant
  still attacked.

- Argon2id for all passwords; tunable cost params from config.
- Our tokens (session, signup, reset, OAuth state) are HS512 under a named key (`kid`), quantum-safe; only HS512 is accepted on decode, and only from a configured key, never by trying others. Keys rotate via `JWT_SIGNING_KEY` / `JWT_TRUSTED_KEYS` (OT-009), and `crates/platform/src/crypto/token.rs` enforces uniform `typ`, `iss`, `aud`, `iat`, and `exp` rules (plus `sub` on user-bound tokens) across all four token kinds. Note for first production rollout: any signup, reset, or OAuth-state tokens minted before centralized `iss`/`aud`/`typ` enforcement won't verify after deploy.
- Outgoing TLS (Google, Turnstile, SMTP) offers post-quantum X25519MLKEM768 first, via the process-wide provider installed at the top of `main` (`crates/platform/src/tls.rs`); `tests/tls_key_exchange.rs` fails if it's lost (OT-009).
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
- Every auth cookie is `__Host-` prefixed (`cookies.rs` holds the names):
  `Secure`, `Path=/`, no `Domain`, `SameSite=Lax`, and `HttpOnly` except the
  CSRF cookie. Browsers accept such a cookie only from this host over HTTPS,
  so a sibling host or a plain-HTTP response can't plant a session, a nonce or
  a CSRF token (OT-010 slice 5, threat model P-22, P-24). It's `Secure` in dev
  too: browsers treat `http://localhost` as secure (Chromium and Firefox;
  Safari may need HTTPS). The edge strips `Cookie` from requests it proxies to
  Umami (`/stats/`), since `Path=/` sends the session cookies everywhere.
- Login brute-force throttling is **handled at the edge, by design** — there is no
  per-account `failed_login`/`locked_until` counter in `login_user.rs`. Password login
  is meant to be protected by (a) a reverse-proxy rate limit on `/api/v1/auth/*` and
  (b) Cloudflare Turnstile on the login form. The rate limit isn't built yet: it's
  planned for the production edge (Traefik), and until then it's the accepted gap
  P-03 in the threat model. High-entropy
  passwords (Argon2id-verified) don't need per-account lockout the way the low-entropy
  6-digit reset/verify codes do — those keep their own atomic attempt cap. If we ever
  drop the edge limiter or Turnstile, revisit this and add an in-app per-account/IP lockout.
- OAuth test bypass (`TEST_OAUTH_BYPASS`) is gated three ways: `#[cfg(debug_assertions)]`
  at compile time, the `TEST_OAUTH_BYPASS` env var at runtime, **and** a startup
  assertion in `main.rs` that refuses to run the bypass unless `DEV_MODE=true`. It is
  therefore absent from a normal release build and cannot run in production.
