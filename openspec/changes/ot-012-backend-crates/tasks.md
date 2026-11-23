# Tasks

## 1. Slice 1 (first slice): workspace with domain, shared and application extracted

Delivers these scenarios: Domain code reaching outward does not compile;
Adding an outer library to an inner layer fails the gate; Application code
reaching into an adapter does not compile; A new unit must be
placed in the allowed list; The allowed list cannot point outward; The API
description is unchanged; Every existing test still runs and passes; The release
image behaves as before; The database schema is unchanged; A lint failure in any
unit fails the gate; A new unit is checked automatically; A stale offline query
cache is still caught; Existing commands still work; Generated files are written
to the same paths.

Everything in `infrastructure/` and `main.rs` moves as one `server`
crate for now (design D1 end state comes in later slices). This puts the sqlx
code in a non-root crate with the cache at the workspace root, which retires
the riskiest unknown.

- [ ] 1.1 Baseline, before touching anything:
  - Save `cargo nextest list --workspace` output (test names and count).
  - Save a `pg_dump --schema-only` of a freshly migrated database.
  - Copy `backend/openapi.json`.
  - Capture the stdout JSON logs of `just smoke` against the local stack
    (one level and message per line), and the span names the collector
    receives.
  - Record the build timings from design.md Hypothesis (clean; handler,
    repository and domain `touch` rebuilds; `just test` / `just test-integration`
    `--no-run`; warm and cold release image build) in design.md.
  - Verify the numbers are in the Hypothesis table.
- [ ] 1.2 Turn `backend/Cargo.toml` into a virtual workspace manifest (D2):
  - members `crates/*` and resolver 3
  - `[workspace.package]` with `version`, `edition = "2024"` and
    `publish = false`, inherited by every member
  - `[workspace.dependencies]` for every third-party crate
  - `[workspace.lints]` carrying `unexpected_cfgs` and the clippy groups now
    passed in the justfile, with the groups at `priority = -1`
  - profiles unchanged

  Verify `cargo metadata` lists the members.
- [ ] 1.3 `git mv` the code into the crates (D8: one move-only commit):
  - `src/shared` → `crates/shared`
  - `src/domain` → `crates/domain`
  - `src/application` → `crates/application`
  - everything else (lib + `main.rs`, `tests/` except `fixtures/`, `benches/`,
    `examples/`) → `crates/server`

  Verify `git log --follow` on a moved file shows its history.
- [ ] 1.4 Fix-up commit:
  - per-crate manifests using `workspace = true` deps and lints
  - `crate::domain::` → `domain::` (likewise application, shared)
  - the `double_must_use` allow at each async_trait crate root
  - `pub(crate)` widened only where the compiler requires
  - the binary still named `overkill-todo`
  - `APP_TARGETS` in server: every lib and bin target, so including
    `overkill_todo` for `main.rs` (D3)
  - `telemetry::export_filter(targets, extra)` added. `telemetry::subscriber`
    takes the export filter as a parameter, and `telemetry::init` takes the
    targets and extra directives.
  - the `overkill_todo=…` test filters (`tasks/job.rs`,
    `telemetry/log_format.rs`, `telemetry/http.rs`, and the `telemetry`,
    `client_telemetry` and `auth_metrics` integration tests) pass
    `APP_TARGETS` (plus the statement directive where they assert on it)
    or the crate's own name
  - a `RUST_LOG` note in `README.md` and `.env.example`
  - `test-support` features (D4): `domain` for `UserBuilder`, enabled from
    application's dev-deps; `application` for `LogCapture`, enabled from
    server's dev-deps
  - outward rustdoc links in application ports become code spans, and the
    `shared::fingerprint` doctest uses `shared::…`
  - `swagger-ui` feature on the server crate

  Verify `cargo build --workspace`, `SQLX_OFFLINE=true cargo build`, and
  that `cargo test --workspace --no-run` and `cargo test --doc --workspace`
  build.
- [ ] 1.5 sqlx in the workspace (D5):
  - `sqlx::migrate!` and every `#[sqlx::test(migrations = …)]` point at
    `backend/migrations`
  - `sqlx.toml` lives in `crates/server`
  - `just db-prepare` / `prepare --check` use `--workspace` and still write
    `backend/.sqlx/`

  - `just db-prepare-check` prints `run 'just db-prepare' and commit
    backend/.sqlx/` on failure (D9)

  Verify that running `just db-prepare` leaves `git status` clean. Also verify
  that editing one query without preparing makes `just ci`'s
  `prepare --check` fail with that message (then revert).
- [ ] 1.6 Re-point path-relative references:
  - fixture `include_str!`s in application telemetry tests
  - the `apps/web/src/pages` walk

  (`model_conformance` reads `MODEL_TRACES_DIR` from the environment, so
  it needs no change.) Verify each test still asserts its input is present,
  and passes.
- [ ] 1.7 Move the `just` recipes, CI (`.github/workflows/ci.yml`) and the
  pre-commit hook to workspace-wide invocations (D9):
  - `nextest --workspace`
  - `clippy --workspace`
  - `fmt --all`
  - `llvm-cov --workspace`
  - `kani --workspace`
  - `bench -p server`
  - `run -p server` in `just dev`, in both the cargo-watch command and
    the plain `cargo run` fallback
  - `export_openapi` via `-p server` in the justfile and in CI's "API spec
    current" step (it moves to the api crate in a later slice)
  - `scripts/models.sh`: `nextest run -p server --test model_conformance`
  - CI release-feature clippy:
    `--workspace --lib --bins --no-default-features`
  - CI offline-cache step: `cargo sqlx prepare --workspace --check` with an
    `::error::` fallback naming `just db-prepare` (the job has no `just`)
  - CI `verify` job: Kani action `args` gain `--workspace`
  - `.gitignore`: `backend/**/proptest-regressions/`
  - `.husky/pre-commit`: both path patterns (the backend trigger and the
    API-spec reminder) match the crate layout
  - `.github/workflows/bench.yml`: path filters on `backend/crates/**`,
    and the warm-cache build uses `-p server`
  - `scripts/bench-compare.sh` handles a base checkout from before the
    split and a PR from after it (D9)

  Verify that every backend recipe named in `CLAUDE.md` and `README.md` runs
  with no new arguments. Verify that `just models` replays its traces. Verify
  that `scripts/bench-compare.sh` runs with `main` from before the split as
  the base.
- [ ] 1.8 Dockerfile: switch both stages to cargo-chef, pinned and `--locked`
  (D7). Keep `.cargo/config.toml` before the dependency build in the dev stage.
  Build the release with `-p server --no-default-features`, and give
  `cargo chef cook` exactly the same profile, package and feature flags as
  the build after it, in both stages. Add a planner stage
  (`COPY . .` then `cargo chef prepare`), and copy only its `recipe.json`
  into the builder and dev stages before cook. In the dev stage, keep
  the registry `chmod` on the cook layer before `CARGO_NET_OFFLINE`, drop
  `touch src/main.rs src/lib.rs`, build with `-p server`, and run
  `cargo watch -x 'run -p server'`. Verify:
  - `just` image build succeeds
  - the image starts against a migrated database and passes `/health/ready`
  - `just smoke` against the local stack passes
  - a source-only change rebuilds without recompiling deps
  - the dev image's live recompile still works
- [ ] 1.9 Crate edge gate (D6): add `crates/server/tests/crate_graph.rs`:
  - it reads `cargo metadata --no-deps --offline --locked`
  - the rank table (shared/domain 0, application 1, server 4; later crates
    added as they appear), checked for normal, build and dev edges, and
    every edge goes to a strictly lower rank
  - the third-party allowlists for `domain` and `application`, normal and
    dev kept apart, taken from their manifests at the split
  - every lib and bin target name (with `-` turned into `_`) is in
    `APP_TARGETS`

  Verify locally, reverting each change after:
  - adding `server` as a dependency of `domain` fails it
  - adding `shared` as a dependency of `domain` fails it (same rank)
  - adding `server` as a dev-dependency of `application` fails it
  - adding `axum.workspace = true` to `domain` fails it, naming domain and
    axum
  - adding an unlisted dummy member fails it
  - a member missing from `APP_TARGETS` fails it
  - editing the table to point outward fails it
- [ ] 1.10 Check that nothing observable changed:
  - `just api-types` leaves `openapi.json` and `schema.d.ts` byte-identical
  - the schema dump matches the 1.1 baseline
  - `cargo nextest list --workspace` has the same tests as baseline (names
    may gain a crate prefix, and the count must not drop)
  - a deliberate clippy violation in `domain` fails `just lint`
    (then revert)
  - the `just smoke` logs and exported span names match the 1.1 baseline
    in level and message (only the `target` field may differ)
- [ ] 1.11 Re-record the Hypothesis timings after slice 1 and write what they
  show in design.md. If the warm or cold image build is more than 10% slower,
  revisit D7 before slice 2.
- [ ] 1.12 Update `backend/ARCHITECTURE.md` (layout tree, "Adding a use case",
  "Adding a repository method" paths), `CLAUDE.md` backend rules and
  `README.md` paths to the workspace layout. Re-point the security tooling
  (D9):
  - every `backend/src/…` and `backend/tests/…` path in
    `docs/security/threat-model.md`
  - the `// Checks:` lines in `models/auth_protocols.spthy`
  - the `backend/tests/*.rs::*` pattern and its resolver in
    `scripts/threat-model-check.sh`
  - `backend/README.md`'s layout section, and the path comments in
    `models/attempt_claim.qnt` and `models/refresh_rotation.qnt`

  Verify `just threat-model-check` and `just protocols` pass, and that
  `grep -rn 'backend/src/\|backend/tests/[a-z_]*\.rs'` over docs, models
  and scripts finds nothing stale.
- [ ] 1.13 `just ci` green. No deploy exists yet. Note for the first deploy:
  the image build now uses cargo-chef, and the binary name and path are
  unchanged.

## 2. Feature done

- [ ] 2.1 After slice 1, revise design.md with what it showed, then add the
  remaining slices here, in this order:
  - (a) Untangle the cross-adapter edges (D3) and extract `platform`
    (D1). Before the move:
    - the HTTP span/metrics middleware, `HEALTH_ROUTES` and
      `UNTRACED_ROUTES` leave `telemetry/http.rs` for the api module
      (still in `server` until (c))
    - `settings.rs` uses `http::HeaderValue`
    - `Persistence` stays in `server` until (b)

    Add `platform`'s `test-support` feature (D4), covering the crypto and
    keyring helpers and `metrics::testing`. Enable `application`'s
    `test-support` from platform's dev-deps (for `LogCapture` in the
    client-reports tests). Verify that `cargo tree -p platform -e normal`
    shows no axum or tower-http.
  - (b) Extract `persistence-postgres`, moving `Persistence`, its
    adapter-level tests (D4), `sqlx.toml` and `model_conformance`
    (`scripts/models.sh` switches to `-p persistence-postgres`), plus
    the schema tests from `audit_log_integration`.
    - Drop `From<sqlx::Error> for DatabaseError` and use
      `.map_err(DatabaseError::new)?` at its sites (D3).
    - Replace every SQL statement in server's tests and benches with
      fixtures from its `testing` module (D5), including the `http` bench's
      database setup and the whole `dump_database` helper.
      `tasks_integration` uses `LeaderLocks` instead.
    - Delete the doc comments in server tests that mention `sqlx::query*`.

    Add three checks to the edge gate (D6): the `sqlx::`-path scan (comments
    stripped, braced and glob imports rejected) with its allowlist, the rule
    that only this crate has `sqlx` as a normal dependency, and the
    unused-fixture check (`pub fn` and `pub async fn`). Verify that each of
    these in a server test fails the scan: a `raw_sql` call, an imported
    `query(…)`, a `use sqlx::{PgPool, query}` and an
    `Executor::execute(&pool, "…")`. Also verify that an uncalled fixture
    fails the gate. This covers "SQL runs only in the database adapter".
  - (c) Extract `services` and `api` (`export_openapi` moves
    to api, and CI's "API spec current" step uses `-p api`), leaving
    `server` as wiring, tasks, full-stack tests and benches. `api` has no
    default features, and server depends on it with
    `default-features = false` (D2). Verify that a clippy warning under
    `#[cfg(not(feature = "swagger-ui"))]` in `api` fails CI's
    release-feature clippy step.

  Each slice adds its crates to the edge table and to `APP_TARGETS`. It
  re-points the threat-model, Tamarin `Checks:` and pre-commit paths for the
  files it moves, and repeats the 1.10 log comparison. Each slice covers "An adapter
  depending on another adapter fails the gate" once both adapters exist.
  Slice (c) checks the two "Editing one adapter" scenarios:
  - after a full build, touch one repository query and run
    `cargo build --workspace -v`, then confirm only `persistence-postgres`
    and `server` show `Compiling`
  - do the same for one handler, expecting only `api` and `server`
- [ ] 2.2 Record the final Hypothesis timings and conclusion (whether finer
  splits are worth it) in design.md.
- [ ] 2.3 Archive the change.
