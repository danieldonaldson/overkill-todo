# OT-012: Backend split into crates

## Why

As the maintainer, I want the backend's layering checked by the compiler rather
than by review, and a change to one adapter to rebuild only that adapter, so
the architecture can't drift quietly and the edit-build-test loop stays quick
as the codebase grows.

The backend is one ~37k-line crate. Its hexagonal rule (domain ← application ←
adapters) is followed today: research into replacing the database driver found
the driver already confined to one module. But nothing enforces the rule. A
`use` of the HTTP framework or the driver in the domain compiles fine and is
caught only if someone notices in review. Every edit also recompiles the whole
crate, so changing a SQL query rebuilds the HTTP handlers, and changing a
handler rebuilds the repositories. The layers are already clean, so splitting
them now is cheap. It gets more expensive with every feature that lands in the
single crate.

## What Changes

- The backend becomes a workspace of separately built crates: one per inner
  layer (domain, application), one per adapter group (HTTP API, database,
  external services, shared platform pieces such as config, crypto and
  telemetry), and a server that wires them together and owns the binary.
- Dependencies between crates point inward only. A gate check also rejects
  any crate dependency that isn't on the allowed list, including one adapter
  depending on another, and any third-party library the inner layers aren't
  allowed to use.
- The few places where adapters reach into each other are untangled, so each
  adapter depends only on the inner layers and the platform crate.
- Tests move to the crate whose code they exercise. Full-stack HTTP tests stay
  together with the app wiring.
- Developer commands (`just …`), committed generated files (the offline query
  cache and the API description) and the release image stay where they are and
  behave as they do now.
- No API, schema, frontend or runtime behaviour changes. Default logs and
  traces stay the same. The one change for operators is that a hand-set
  `RUST_LOG` names the new crates rather than `overkill_todo`.

## Capabilities

### New Capabilities

- `architecture`: the backend's layering is enforced by the build and the
  gate, the split leaves every observable behaviour unchanged, and every
  existing check still covers all of the backend.

### Modified Capabilities

None.

## Impact

- Every backend source file moves. Imports change from `crate::…` paths to
  crate names, and some `pub(crate)` items become `pub`.
- Build tooling: the backend manifest becomes a workspace, and the justfile
  backend recipes, CI, the Dockerfile's dependency-caching layer and the dev
  image's live recompile all change. So do clippy/deny/machete/coverage/Kani
  invocations and the sqlx offline-cache preparation.
- Docs: `backend/ARCHITECTURE.md`, `CLAUDE.md` (backend rules, commands) and
  `README.md` where they name paths.
- Security and model tooling that names backend file paths: the threat model
  (`docs/security/threat-model.md`), the Tamarin model's `Checks:` lines and
  `scripts/threat-model-check.sh`, and `scripts/models.sh`. The same goes
  for the bench workflow and `scripts/bench-compare.sh`.
- No change to the API description, database schema, frontend or deploy
  config.

## Out of Scope

- **Changing the database driver.** That question started this change, but
  it's not part of it. The split only makes a driver swap a one-crate change.
- **Moving DTOs out of application** or otherwise redrawing what a layer
  holds. Code moves as it is; only the cross-adapter tangles get untangled.
- **Finer splits** (one crate per adapter service, per use-case group). These
  can come later if build times show they're worth it.
- **Publishing crates** or versioning them separately.
- **The frontend.**

## Dependencies

OT-008 (audit log) had to be finished first, because it touches every layer.
It was archived on 2026-11-21 (`f935763`), so nothing blocks this change.

## Appetite

2 weeks.

## Ready

- **Riskiest unknown:** Whether the tooling built around a single crate
  carries over to a workspace without slowing builds or getting brittle. That
  tooling is the sqlx offline cache and compile-time type mapping, migrations
  for `#[sqlx::test]`, the Dockerfile's dependency-caching layer, the dev
  image's live recompile, and Kani.
- **First slice:** Make the backend a workspace and extract the domain,
  shared and application crates. The rest moves as one crate under the
  workspace, with every `just` command, CI job and image build green. Build
  times are measured before and after.
