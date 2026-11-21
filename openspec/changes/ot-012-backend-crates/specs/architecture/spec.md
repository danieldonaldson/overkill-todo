# Spec Delta

## Purpose

Keeps the backend's structure honest: its layering is enforced by the build and
the gate rather than by review, so the code can't drift from the documented
architecture, and structural changes leave everything clients and the operator
see unchanged.

## ADDED Requirements

### Requirement: Inner layers cannot depend on outer ones

The backend SHALL be built as separate units, so that the domain layer can use
no other layer and no I/O library, and the application layer can use only the
domain layer and no adapter. Breaking either rule SHALL fail to compile, not
merely fail review.

#### Scenario: Domain code reaching outward does not compile

- **WHEN** domain code refers to the application layer, an adapter, the HTTP framework or the database driver
- **THEN** the backend fails to build, and the error points at that reference

#### Scenario: Application code reaching into an adapter does not compile

- **WHEN** application code refers to any adapter (HTTP, database, external services, platform)
- **THEN** the backend fails to build, and the error points at that reference

### Requirement: Adapters depend only on inner layers and the platform

Each adapter SHALL depend only on the domain and application layers and the
shared platform unit (configuration, crypto, telemetry). Only the server,
which wires the application together, SHALL depend on several adapters. The
gate SHALL check every dependency between backend units against an allowed
list.

#### Scenario: An adapter depending on another adapter fails the gate

- **WHEN** a dependency is added from one adapter to another (for example, from the HTTP adapter to the database adapter)
- **THEN** the full gate and CI fail, naming the two units

#### Scenario: SQL runs only in the database adapter

- **WHEN** any other backend unit, including its tests and benchmarks, adds a database query
- **THEN** the full gate and CI fail, naming the file and line

#### Scenario: A new unit must be placed in the allowed list

- **WHEN** a new backend unit is added without an entry in the allowed list
- **THEN** the full gate and CI fail, naming the unit

#### Scenario: The allowed list cannot point outward

- **WHEN** the allowed list is edited to let an inner layer depend on an outer one
- **THEN** the full gate and CI fail, naming the edge

### Requirement: Editing one adapter rebuilds only what depends on it

A change confined to one adapter SHALL NOT recompile the other adapters or the
inner layers.

#### Scenario: Changing a database query leaves the HTTP adapter alone

- **WHEN** only a query in the database adapter changes and the backend is rebuilt
- **THEN** only the database adapter and the server are recompiled

#### Scenario: Changing a handler leaves the database adapter alone

- **WHEN** only an HTTP handler changes and the backend is rebuilt
- **THEN** only the HTTP adapter and the server are recompiled

### Requirement: Restructuring the backend changes no observable behaviour

Splitting the backend into units SHALL leave unchanged everything an API
client, the web app or the operator can observe.

#### Scenario: The API description is unchanged

- **WHEN** the API description is regenerated after the split
- **THEN** it is identical, byte for byte, to the committed description from before the split

#### Scenario: Every existing test still runs and passes

- **WHEN** the backend unit and integration tests run after the split
- **THEN** they all pass, and no fewer tests run than before the split

#### Scenario: The release image behaves as before

- **WHEN** the release image is built and started against a migrated database
- **THEN** it starts, reports ready on its health checks, and passes the smoke checks

#### Scenario: The database schema is unchanged

- **WHEN** migrations run on an empty database after the split
- **THEN** the resulting schema is identical to the one before the split, and no migration was added, edited or reordered

### Requirement: Every backend check covers every unit

Every check the gate runs on the backend (format, lint, unused dependencies,
licence and advisory checks, tests, coverage, the offline query cache and
the bounded proofs) SHALL cover every backend unit. A unit added later SHALL
be covered without editing the gate.

#### Scenario: A lint failure in any unit fails the gate

- **WHEN** code that breaks a lint rule is added to any backend unit
- **THEN** the full gate and CI fail, naming the file

#### Scenario: A new unit is checked automatically

- **WHEN** a new backend unit is added with a lint failure, a test failure or an unused dependency
- **THEN** the full gate and CI fail without any change to the gate's definition

#### Scenario: A stale offline query cache is still caught

- **WHEN** a query changes in any unit and the offline query cache is not regenerated
- **THEN** the full gate and CI fail, saying how to regenerate it

### Requirement: Developer commands and generated files stay where they are

The developer commands SHALL keep their names and meaning, and committed
generated files (the API description and the offline query cache) SHALL stay
at their existing paths, so instructions that name them stay true.

#### Scenario: Existing commands still work

- **WHEN** the developer runs any existing backend command (run locally, test, lint, prepare the query cache, generate API types, benchmark, build images)
- **THEN** it does the same job as before the split, with no new arguments

#### Scenario: Generated files are written to the same paths

- **WHEN** the API description and the offline query cache are regenerated
- **THEN** they are written to the same paths as before, and nothing else is left at an old path
