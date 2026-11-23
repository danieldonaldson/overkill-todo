# Design

## Context

See proposal.md (Why). The backend is one crate (`backend/Cargo.toml`, lib + bin)
with `domain/`, `application/`, `infrastructure/` and `shared/` modules. Survey
before writing this (2026-11-20):

- **Inner layers are already clean.** No `crate::infrastructure` import exists
  in `application/` or `domain/`. The only mentions are rustdoc links. Domain
  uses only `jiff`, `uuid` and `thiserror`.
- **Mocks are local.** Every `#[cfg_attr(test, mockall::automock)]` is on an
  outgoing port, and only application use-case tests use the mocks.
- **The orphan rule holds.** Every foreign-trait impl in infrastructure is on a
  local type or has the form `From<Local> for Foreign`, so nothing breaks when
  domain and application types move to other crates.
- **Infrastructure modules have three tangles** that would block crate
  boundaries:
  - `persistence ↔ telemetry`: the audit-log repository calls
    `telemetry::http::current_trace_id`. Telemetry imports
    `persistence::STATEMENT_LOG_DIRECTIVE` and `DatabaseHealth`.
  - `api → persistence::Database`: the concrete driver alias, used in
    `state.rs`, `routes.rs` and the health handler.
  - `services → api`: one `jwt_service` test cross-checks against
    `api::create_jwt`. It's test-only.
  - `config ↔ crypto` is a rustdoc link only.
- **`DatabaseHealth` can't be a trait object.** It uses
  return-position `impl Future`.
- **Tests and benches:** 27 integration test files, 4 benches and 1 example
  import `overkill_todo::…`. Most use the `tests/common` HTTP harness. Every
  `#[sqlx::test]` already names `migrations = "./migrations"` explicitly.
  15 full-stack test files and the `http` bench run about 82 raw SQL
  statements (17 `query!`, 65 runtime `sqlx::query(…)`). None of them is what
  the test is about. They are setup and assertion plumbing: seeding rows
  (users, OAuth accounts, codes, audit events), moving time
  (`expires_at = NOW() - INTERVAL …`, back-dated deletions) and probing state
  (`COUNT(*)` of tokens and codes, `revoked_at`, `information_schema.tables`).
- **Paths others depend on:** the frontend guard and `CLAUDE.md` name
  `backend/tests/fixtures/client_islands.json`. Two application unit tests
  `include_str!` fixtures by relative path, and one test walks
  `../apps/web/src/pages` from `CARGO_MANIFEST_DIR`. Outside `backend/`
  these name backend paths or run single-crate cargo commands:
  - `docs/security/threat-model.md`: about 170 `backend/src`/`backend/tests`
    paths
  - `models/auth_protocols.spthy`: `// Checks: backend/tests/<file>.rs::<fn>`
    lines
  - `scripts/threat-model-check.sh`: enforces the
    `backend/tests/*.rs::*` shape
  - `scripts/models.sh`: `cargo nextest run --test model_conformance`
  - `.github/workflows/bench.yml`: path filters on `backend/src/**`,
    `backend/benches/**` and `backend/tests/common/**`
  - `scripts/bench-compare.sh`: checks `backend/benches/<b>.rs`. It
    also compares a base checkout against the PR, so for one PR the two
    sides have different layouts.
  - `.husky/pre-commit`: `backend/(src|tests)/` triggers, and the
    `backend/src/(application/dto|infrastructure/api)/` API-spec reminder
  - the dev stage of `backend/Dockerfile`: `touch src/main.rs src/lib.rs`
  - `.github/workflows/ci.yml`: release-feature clippy and a direct
    `cargo sqlx prepare --check`

  `model_conformance` reads its traces from `MODEL_TRACES_DIR`, not a
  relative path.
- **Log and trace targets are the crate name.** The default and export
  filters (`telemetry/mod.rs`), and test filters in `tasks/job.rs`,
  `telemetry/log_format.rs` and three telemetry integration tests, hardcode
  `overkill_todo=…`. After the split, events carry `domain::…`, `api::…` and
  so on, and those filters would silently drop every app log and span.
- **Some platform code uses axum.** The HTTP span/metrics middleware in
  `telemetry/http.rs` uses axum and tower-http, and `config/settings.rs`
  parses CORS origins with `axum::http::HeaderValue`. `current_trace_id`
  in the same file uses only tracing and OpenTelemetry.
- **`Persistence` holds the concrete driver.** Its `db: Arc<Database>`
  field names the postgres alias, and only `main.rs` uses the struct.
- **Cross-crate test helpers are `#[cfg(test)]`:** `Crypto::for_testing`,
  `Crypto::for_testing_with_tokens`, `TEST_RSA_PRIVATE_KEY` (used by the
  api and Google OAuth tests) and `telemetry::metrics::testing` (used by the
  tasks tests). `#[cfg(test)]` items aren't visible to other crates' tests.
- **The `http` bench builds its own database.** It uses
  `sqlx::migrate::MigrateDatabase`, `PgPool::connect` and
  `sqlx::migrate!("./migrations")`, as well as runtime queries.

## Goals / Non-Goals

**Goals:**

- Map each existing module onto a crate without changing what it does. The
  only exceptions are untangling the three edges above.
- Keep module paths inside each crate the same, so an import changes only
  its prefix (`crate::domain::entities::User` →
  `domain::entities::User`).

**Non-Goals:**

- Removing `async_trait`, moving `AppError`, or tidying APIs while files move.
  Moves and edits stay in separate commits.
- Splitting `platform` or `services` further.

## Decisions

### D1. Eight crates, in `backend/crates/`

```
                     shared            domain
                        ^                 ^
                        +-----+   +-------+
                              |   |
                           application
                                ^
                                |
                             platform
           (config, crypto, tls, telemetry, persistence traits)
               ^                ^                ^
               |                |                |
     persistence-postgres    services           api
     (sqlx, the only         (email, argon2,    (axum, handlers,
      driver user)            JWT, Turnstile,    middleware, OpenAPI)
                              MX, Google OAuth)
               ^                ^                ^
               +----------------+----------------+
                                |
                              server
           (bin `overkill-todo`, main.rs wiring, background tasks,
            full-stack tests, benches)
```

Every crate may also depend on any crate below it in this picture. Adapters
never depend on each other.

| Crate | Holds (today's module) | Workspace deps |
|---|---|---|
| `shared` | `shared/` | — |
| `domain` | `domain/` | — |
| `application` | `application/` (DTOs, ports, use cases) | domain, shared |
| `platform` | `infrastructure/{config, crypto, tls, telemetry}`, and the driver-neutral root of `persistence/` | domain, application, shared |
| `persistence-postgres` | `infrastructure/persistence/postgres/` | domain, application, platform |
| `services` | `infrastructure/{services, oauth}` | domain, application, platform |
| `api` | `infrastructure/api/` | domain, application, platform, shared |
| `server` | `main.rs`, `infrastructure/tasks/`, wiring | all of the above |

The driver-neutral persistence root covers `DatabaseHealth`, `LeaderLocks`,
`DatabaseMaintenance`, `DatabaseError` and `PoolStats`. `Persistence`
holds the concrete `Database`, so it moves to `persistence-postgres`: it's what
that adapter hands the server, which is its only user.

`platform` does not depend on axum or tower-http:

- The HTTP span/metrics middleware in `telemetry/http.rs` moves to `api`.
  `current_trace_id` and the untraced-route list stay in
  `platform::telemetry`, because the postgres audit adapter and the api
  `trace_id` middleware both read them.
- `config/settings.rs` checks CORS origins with `http::HeaderValue` (the
  `http` crate) in place of the axum re-export.

Otherwise D4's "a query edit reruns its repository tests without building
axum" wouldn't hold, since `persistence-postgres` depends on `platform`.

- **Why `platform`:** config, crypto and telemetry are needed by every adapter.
  The neutral persistence traits are needed by api (readiness), telemetry
  (pool gauges) and tasks (leader locks), and must not live in the postgres
  crate or those consumers would rebuild on every query edit. With telemetry
  in `platform`, the postgres audit adapter can read `current_trace_id`
  without an adapter→adapter edge.
- **Why tasks live in `server`:** they wire use cases, leader locks and
  telemetry together under the leader lock. That's composition, the same job
  as `main.rs`.
- **Names:** packages, libs and folders use the bare name (`domain`,
  `crates/domain`, `use domain::…`). There's no prefix, because nothing is
  published and none of the names matches a package in `Cargo.lock`
  (checked 2026-11-21). If a future dependency brings in a crates.io package
  with the same name, `cargo -p <name>` becomes ambiguous. Fix that by
  renaming our crate then (`package = …` in the dependents keeps the `use`
  paths), not by prefixing everything now. The binary keeps the name `overkill-todo`, so the
  Dockerfile's `target/release/overkill-todo` and every script that runs it
  are unchanged.
- **Alternatives:** three crates mirroring the layers (shape A). It's simpler,
  but adapters still rebuild each other and nothing stops adapter→adapter
  edges. One crate per adapter service (~12 crates) means more manifests and
  more `pub` surface, for rebuild gains we haven't measured. Telemetry as its
  own crate is possible later. Today it would have to sit below postgres
  anyway, so it buys nothing.

### D2. `backend/` stays the workspace root

`backend/Cargo.toml` becomes a virtual manifest (`members = ["crates/*"]`,
resolver 3, `[workspace.dependencies]`, `[workspace.lints]`, `[profile.*]`).
Everything else other tools name stays at its path:

- `Cargo.lock`, `target/` and `rust-toolchain.toml`
- `clippy.toml`, `deny.toml` and `.cargo/config.toml`
- `migrations/`, `.sqlx/` and `openapi.json`
- `tests/fixtures/` and `.env`

Commands still run from `backend/`, so `dotenvy` finds `.env` as before.

- `[workspace.package]` holds `version`, `edition = "2024"` and
  `publish = false`, and every member inherits them. So
  `env!("CARGO_PKG_VERSION")` (the reported service version, read in
  `config/settings.rs`) stays the single workspace version wherever config
  lives.
- Third-party versions move to `[workspace.dependencies]`, and members use
  `foo.workspace = true`. Version bumps stay one-line.
- The clippy groups (pedantic, nursery, `allow_attributes_without_reason`) and
  `unexpected_cfgs` move into `[workspace.lints]`, so editor `cargo clippy`
  matches the gate. The justfile keeps `-D warnings` and adds `--workspace`.
  Every crate that uses `async_trait` repeats the `double_must_use` allow
  (with its reason) at its root.
- `swagger-ui` stays an optional dep of `api`. `server`
  re-exposes it as its default feature
  (`swagger-ui = ["api/swagger-ui"]`), and release builds still pass
  `--no-default-features`.

### D3. Untangling the cross-adapter edges

- **`STATEMENT_LOG_DIRECTIVE`** is driver-specific (it names sqlx's log
  target). It moves to `persistence-postgres`, and the server passes
  it into telemetry init as an extra filter directive. Telemetry no longer
  knows the driver.
- **`Database` alias** lives only in `server`, so only the server
  knows which driver is selected.
- **The api crate** defines a small object-safe `ReadinessCheck` trait
  (boxed future), with a blanket impl for every `T: DatabaseHealth`.
  `AppState` and `RouterDeps` hold `Arc<dyn ReadinessCheck>`. That costs one
  allocation per `/health/ready` call.
  - Alternative rejected: making `AppState` generic over the database. The
    parameter would spread to every handler's `State` and to the OpenAPI
    builder, which must build without a database.
- **Telemetry pool gauges** are already generic over `impl DatabaseHealth`.
  No change needed.
- **The `jwt_service` vs `create_jwt` cross-check** spans two adapters, so it
  moves to `server`'s tests.
- **Log and trace filters name every workspace crate.** `server` holds the
  list of workspace crate names (`APP_TARGETS`) and passes it to
  `telemetry::init`. Init builds the default and export filters from that
  list (`domain=info,application=info,…,tower_http=info`) plus the
  driver's directive, so platform never names the other crates.
  - `crate_graph.rs` (D6) fails if a workspace member isn't in
    `APP_TARGETS`, so a new crate's logs can't silently go missing.
  - Server's tests and tasks build their filters from the same list. Tests
    inside one crate filter on that crate's own name
    (`env!("CARGO_CRATE_NAME")`).
  - Changed for operators: a hand-set `RUST_LOG=overkill_todo=debug` no
    longer matches anything. `RUST_LOG` names crates now
    (`RUST_LOG=api=debug`). The README and `.env.example` say so. No
    deployed config sets it (checked 2026-11-23), and no deploy exists yet.
  - Alternative rejected: lib names prefixed `overkill_todo_*`. That keeps
    the old filter matching by prefix, but splits each crate between two
    names (lib vs. dependency key).

### D4. Tests go to the lowest crate that has everything they use

- **Unit tests** stay inline in their files, so they move with the code. The
  mockall mocks stay in `application`, behind `cfg_attr(test, …)`,
  because nothing outside it uses them.
- **Adapter-level integration tests** go to that adapter's `tests/` when they
  need only the adapter and inner crates. They take a dev-dependency on
  `platform` with its `test-support` feature, for `Crypto::for_testing`.
- **`platform` gets a `test-support` feature.** Today
  `Crypto::for_testing`, `Crypto::for_testing_with_tokens`,
  `TEST_RSA_PRIVATE_KEY` and `telemetry::metrics::testing` are
  `#[cfg(test)]`, so other crates' tests can't see them. They move behind
  `#[cfg(any(test, feature = "test-support"))]`, and the feature also
  enables `opentelemetry_sdk/testing`. Like every `test-support` feature,
  it's enabled only from `[dev-dependencies]`.
  - Postgres adapter: `attempt_lockout`, `database_maintenance`,
    `model_conformance`, and `repository_integration` if its use of `common`
    is only crypto.
  - Platform: `tls_key_exchange`.
  - Benefit: a query edit reruns its repository tests without building axum.
- **Full-stack tests** (anything using the `tests/common` HTTP harness or
  more than one adapter) go to `server/tests/` along with `common/`.
  That's most files, including `tasks`, `telemetry_metrics`, `token_keys` and
  `route_guards`. The server is the composition root, and these tests
  exercise the assembled app.
  - Alternative rejected: a separate `integration-tests` crate. It would
    depend on everything the server does and rebuild just as often, only
    with one more manifest.
- **Database fixtures** for full-stack tests come from the postgres crate's
  `testing` module (D5), never from SQL written in `server`.
- **Shared test helpers:** if an adapter's tests end up needing harness
  pieces, they go in a dev-only `testkit` crate (dev-dependency
  only, never in the release graph). It's added only when the first
  duplicate appears. Test-only constructors that must cross crates go behind
  a `test-support` feature, enabled only from `[dev-dependencies]`.
- **Benches:** all four stay together in `server/benches/`. They
  share `common` and span adapters, so `just bench` is unchanged. The
  `http` bench's database setup (create, connect, migrate) and its runtime
  queries move into `persistence-postgres::testing` (D5), so `server`
  never calls `sqlx::migrate!` or `MigrateDatabase`.
- **The `export_openapi` example** moves to `api/examples/`.
  `api_routes()` takes no values, so `just api-types` stops building the
  postgres and services crates.
- **Fixtures** stay at `backend/tests/fixtures/` (frontend guard,
  `CLAUDE.md`), and `models/` traces stay where `just models` writes them.
  `include_str!` and `CARGO_MANIFEST_DIR` paths are re-pointed, and each
  path-reading test already fails loudly when its file is missing.
- **Counting:** before the split, `cargo nextest list` output is saved. After
  each slice, the test count must not drop (spec: "Every existing test still
  runs and passes").

### D5. sqlx in a workspace

- `migrations/` stays at `backend/migrations`. The postgres crate uses
  `sqlx::migrate!("../../migrations")`. Every
  `#[sqlx::test(migrations = …)]` is re-pointed to the same relative path,
  as a mechanical edit.
- The offline cache stays at `backend/.sqlx/`, via
  `cargo sqlx prepare --workspace -- --all-targets` (and `--check` in
  `just ci` and CI). The `CLAUDE.md` rule "commit `backend/.sqlx/`" stays
  true.
- **SQL lives only in `persistence-postgres`, including test SQL.**
  The crate gets a `testing` module behind a `test-support` feature, holding
  named fixtures for what the tests do today:
  - seeders: `seed_user(…)`, `seed_oauth_account(…)`, `seed_reset_code(…)`,
    `seed_audit_event(…)`
  - time travel: `expire_verification_codes(user)`,
    `expire_refresh_tokens(user)`, `backdate_deletion(user, by)`
  - probes: `count_refresh_tokens(user)`, `refresh_token_revoked_at(id)`,
    `table_names()`

  Each fixture is a checked `query!`, so the 65 runtime queries become
  compile-checked too, in line with the "`query!` only" rule. A schema change
  then breaks only this crate, never the HTTP tests. `server` enables
  the feature only from `[dev-dependencies]`, so the fixtures never reach the
  release graph. The fixtures are `pub` items in a library, so `dead_code`
  never flags one that no test calls. The edge gate (D6) checks that instead.
- **`sqlx` stays a dev-dependency of `server`,** because
  `#[sqlx::test]` (a fresh database per test) and `PgPool` expand to
  `::sqlx` paths. Server compiles no SQL, so there's one `sqlx.toml` (the
  `TIMESTAMPTZ → jiff_sqlx::Timestamp` override, read from the compiling
  crate's directory) and every `.sqlx/` entry comes from one crate.
- Alternatives rejected:
  - Keeping the test SQL in `server` with a second `sqlx.toml` and an
    identity test. Table and column names would leak into 15 HTTP test
    files, and a column rename would break them.
  - Fixtures through the repository ports only. The ports can't move time
    or read columns the API never exposes (`revoked_at`, row counts), and
    adding those to production ports would widen them for tests.

### D6. Gate check for crate edges

A test in `server` (`tests/crate_graph.rs`) runs
`cargo metadata --no-deps --offline --locked` (the declared dependencies are
enough, and it needs no network) and checks four things:

- **Workspace edges.** Every workspace→workspace dependency, of every kind
  (normal, build and dev), is checked against a table in the test. Dev
  edges follow the same rank rules as the others. The one extra they allow is
  `testkit`.
- **Third-party dependencies of the inner layers.** `domain` and
  `application` each have an allowlist of the third-party crates they may
  depend on, taken from what they use at the split (domain: `jiff`, `uuid`,
  `thiserror`). A new entry needs a deliberate edit to the test. Without
  this, adding `axum.workspace = true` to domain's manifest would compile, and
  the "Domain code reaching outward does not compile" scenario would not hold.
  `sqlx` may be a normal dependency only of `persistence-postgres`, and a dev
  dependency only of `server`.
- **SQL outside the database adapter.** The test scans the sources, tests
  and benches of every crate except `persistence-postgres`. It fails on any
  `sqlx::` path or `use sqlx` import that isn't on a short allowlist
  (`sqlx::test`, `sqlx::PgPool`, `sqlx::postgres::PgPoolOptions` where
  the harness needs it), naming the file and line. That catches
  `query*`/`query_as*`, and also `raw_sql`, `Executor::execute(&pool, "…")`
  and imported `query(…)`. Server's dev-dependency on `sqlx` exists only for
  `#[sqlx::test]`, so nothing in the dependency graph stops it running SQL.
- **Unused fixtures.** Every `pub fn` in `persistence-postgres`'s `testing`
  module is called somewhere in another crate's tests or benches.
- **Log targets.** Every workspace member is listed in server's
  `APP_TARGETS` (D3).

Each crate in the table has a rank:

| Rank | Crates |
|---|---|
| 0 | shared, domain |
| 1 | application |
| 2 | platform |
| 3 | adapters |
| 4 | server |

The test fails when:

- an edge isn't in the table
- a workspace member has no row
- any edge in the table goes from a lower rank to a higher one, or between
  two rank-3 adapters

It runs under `just test-integration`, which is already in `just ci`.

- Alternative: cargo-deny `[bans]` with `wrappers`. It's already in the
  gate, but it can't express "new crate must be listed" or "table can't point
  outward", and rules for the workspace's own crates read oddly there.
  `wrappers` could limit `sqlx` by itself, but an allowlist for domain and
  application can't be written that way. Keeping every rule in one test
  gives one place to look and better error messages.
- Alternative: a `just` recipe with `jq`. It adds a tool dependency, and a
  Rust test reports failures better.

### D7. Docker dependency caching uses cargo-chef

Both Dockerfile stages (release, and dev with live recompile) pre-build deps
from a stub `src/` today. A workspace would need a stub per crate and an
edit for every new crate. Instead, `cargo chef prepare` / `cargo chef cook`
are installed `--locked` at a pinned version, the same way `cargo-auditable`
is. The dev stage keeps the rule that `.cargo/config.toml` lands before the
dependency build. The final `COPY . .` and `cargo build` stay. The release
build becomes `cargo auditable build --release --locked -p server
--no-default-features`.

`cargo chef cook` must use the same profile, package and feature flags as
the build that follows it (`--release --locked -p server
--no-default-features` in the release stage, and the dev stage's flags in
the dev stage). If they differ, the cached deps are built with a different
feature set (for example with swagger-ui), and the real build recompiles
them.

- Alternative: a script that generates stubs from `cargo metadata`. Fewer
  tools, but more of our own code to keep correct.

### D8. Moves are mechanical commits

Each slice does the file move (`git mv`) in one commit, and the path and
visibility fixes in the next, so `git log --follow` and blame survive.
`pub(crate)` widens to `pub` only where the compiler demands it.
`unreachable_pub` stays off. Narrowing visibility is later cleanup, not part
of this change.

### D9. Tooling invocations

All of these go through `just`, so commands keep their names (spec:
"Developer commands and generated files stay where they are").

| Command | Becomes |
|---|---|
| `just test` | `cargo nextest run --workspace --lib` |
| `just test-integration` | `cargo nextest run --workspace --tests` |
| `just lint` | `cargo clippy --workspace --all-targets` |
| `just fmt-check` | `cargo fmt --all --check` |
| coverage | `cargo llvm-cov nextest --workspace --tests` |
| `just verify` | `cargo kani --workspace` (proofs live only in domain today; a crate that adds proofs is covered with no recipe change) |
| `just dev` | `cargo watch -x 'run -p server'` |
| bench | `cargo bench -p server --bench …` |
| `just dev` fallback (no cargo-watch) | `cargo run -p server` |
| `just models` (`scripts/models.sh`) | `cargo nextest run -p <crate> --test model_conformance` (`server` in slice 1, `persistence-postgres` from slice (b)) |
| CI release-feature clippy | `cargo clippy -p server --lib --bins --no-default-features` |
| CI offline-cache check | `just db-prepare-check`, so CI prints the same message as the gate |
| Dockerfile dev stage | `cargo build --locked -p server` after cook (no `touch src/…`), and `CMD cargo watch -x 'run -p server'` |

A bare `cargo run` / `bench` / `clippy` at a virtual workspace root
selects every member, so most of these would still run. Naming `-p`
keeps each one's scope the same as today and stops it from breaking when a
second binary or bench appears.

**Path-matching scripts and docs** are re-pointed in the slice that moves
the files they name, and again in each later slice that moves those files:

- `docs/security/threat-model.md` and the `// Checks:` lines in
  `models/auth_protocols.spthy` name `backend/crates/<crate>/tests/<file>.rs::<fn>`
  and `backend/crates/<crate>/src/…`.
- `scripts/threat-model-check.sh` accepts
  `backend/crates/*/tests/*.rs::*` and resolves the file under that crate.
- `.husky/pre-commit` triggers on `backend/crates/` (any `src/`,
  `tests/` or `benches/`). The API-spec reminder matches the DTO and api
  module paths in their current crate (`crates/application/src/dto/`, and
  `crates/server/src/infrastructure/api/` until slice (c) moves it to
  `crates/api/`).
- `.github/workflows/bench.yml` filters on `backend/crates/**`, and its
  warm-cache build runs `cargo bench --locked --no-run -p server`.
- `scripts/bench-compare.sh` finds each side's bench at
  `backend/crates/server/benches/<b>.rs` or, for a base before the split,
  `backend/benches/<b>.rs`, and adds `-p server` only when the
  workspace layout is found.

`cargo deny check` and `cargo machete` already walk the workspace. Machete
false positives move to each crate's `[package.metadata.cargo-machete]`.

`just db-prepare-check` wraps `cargo sqlx prepare --check`. When the check
fails, the recipe prints `run 'just db-prepare' and commit backend/.sqlx/`,
because sqlx's own hint names `cargo sqlx prepare`, not the recipe (spec: "A
stale offline query cache is still caught").

## Hypothesis

- **Belief:** after the full split, an adapter-only edit (one handler, one
  repository query) gives a clearly faster incremental `cargo build` and
  `just test-integration` compile than today. Domain and application edits
  stay about the same, since everything depends on them. Clean and release
  builds stay within ±10%, since LTO with `codegen-units = 1` dominates the
  release link.
- **How:** before slice 1, record dev build times on the same machine and
  power state:
  - a clean build
  - `touch` on a handler, on a repository and on a domain entity, each
    followed by a rebuild
  - `just test` and `just test-integration` with `--no-run`
  - the release image build, warm and cold

  Record the same numbers after slice 1 (partial: adapters are still one
  crate) and after the last slice. Keep the table here.
- **How we'll know:** if adapter-only edits don't improve after the last
  slice, the change still stands on enforcement (spec requirements 1–2). We
  then don't pursue finer splits. If the release image build gets more than
  10% slower, revisit D7 before going on.

## Risks / Trade-offs

- [sqlx offline cache or type overrides misbehave across crates (macro looks
  in the wrong `.sqlx`, override not applied)] → Slice 1 already moves the
  sqlx-using code into a non-root crate with the cache at the workspace root,
  so it's proven before any further splitting.
- [Docker cache layer regresses image build time] → cargo-chef in slice 1, and
  warm and cold image builds measured against the baseline.
- [Fixtures become a second, test-only repository API that grows without
  limit] → one fixture per distinct test need, named for what it does, in a
  module that only the `test-support` feature compiles. The edge gate (D6)
  fails on a fixture that no test calls. `dead_code` can't do this, because
  the fixtures are `pub`.
- [Wider `pub` surface invites cross-crate reach-ins] → the edge check (D6)
  limits who can depend on whom. Narrowing visibility is later cleanup.
- [Gains smaller than hoped, because most feature edits touch application] →
  measured (Hypothesis). Enforcement is the primary value anyway.
- [Path-relative references (fixtures, web pages walk, migrations) silently
  miss] → every such test asserts its input exists and is
  non-empty. A dropped test count is caught by the nextest list comparison.
- [Parallel worktrees (`just wt-new`) mid-move get hard rebases] → don't keep
  worktrees open across a move commit. Each slice lands quickly.

## Open Questions

- Should `telemetry` leave `platform` once it has more consumers? Only build
  numbers can answer that. It doesn't affect any spec scenario.
