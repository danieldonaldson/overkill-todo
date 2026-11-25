# Spec Delta

## ADDED Requirements

### Requirement: Lists and todos are private to their Member
Every route that reads or changes lists or todos SHALL act only on those of the
account named by the caller's session, whose email address SHALL be verified.
A request naming another account's list or todo SHALL get the same response as
one naming an ID that never existed, and SHALL change nothing. No page or route
SHALL let an Admin or Super Admin see or change another account's lists or
todos. This SHALL be a numbered property in the threat model, with a check.

#### Scenario: Another Member's list
- **WHEN** a Member sends any list or todo request naming another Member's list or todo
- **THEN** the response is the same 404 as for an ID that never existed, and nothing changes

#### Scenario: Another list's todo
- **WHEN** a Member sends a request naming their own list and a todo that belongs to another Member's list
- **THEN** the response is the same 404, and nothing changes

#### Scenario: Positioning next to another Member's todo
- **WHEN** a Member moves their todo to a position relative to another Member's todo
- **THEN** the response is the same as for a neighbour ID that never existed, and reveals nothing about the other todo

#### Scenario: Reused creation ID
- **WHEN** a Member creates a list or todo with an identifier that another Member's list or todo already uses
- **THEN** the other Member's data is neither returned nor changed

#### Scenario: Admin or Super Admin
- **WHEN** an Admin or Super Admin requests another account's list or todo
- **THEN** they get the same 404 as a Member would, and no admin page or route shows list names, titles or counts

#### Scenario: Visitor
- **WHEN** a request without a valid session reaches any list or todo route
- **THEN** it is refused with 401

#### Scenario: Unverified Member
- **WHEN** a signed-in Member whose email address is not verified requests any list or todo route
- **THEN** it is refused as on other routes that require a verified email address

#### Scenario: Missing CSRF token
- **WHEN** a request that changes a list or todo carries a valid session but no valid CSRF token
- **THEN** it is refused and nothing changes

#### Scenario: The property is checked
- **WHEN** the check for this property is removed from the threat model's catalogue or renamed
- **THEN** the full gate and CI fail, naming the property

### Requirement: List guarantees are machine-checked on every change
Each guarantee in the lists capability that concerns ordering, versions, limits,
ownership or deletion SHALL be checked by the strongest method that fits it,
over every input or interleaving within stated bounds:

- a proof over every input within stated bounds for the pure ordering,
  validation and version rules;
- a model checked over every interleaving within stated bounds for the server
  writes and for the client queue;
- a conformance test that replays the model's traces through the real code.

Each guarantee that cannot be checked formally SHALL name its test and state why
no proof or model applies. These checks SHALL run in the full local gate and in
CI. Each SHALL state its bounds and assumptions next to it. A change that breaks
a guarantee SHALL fail the build with a concrete counterexample or trace.

#### Scenario: Breaking a guarantee fails the gate
- **WHEN** a change makes a list guarantee false for some input or interleaving within its bounds
- **THEN** the full gate and CI fail, naming the guarantee and a counterexample or trace

#### Scenario: A removed guard is noticed
- **WHEN** a guard that enforces a modelled guarantee is removed
- **THEN** the broken variant produces a counterexample, so the guarantee is not vacuously true

#### Scenario: A model that drifts from the code fails replay
- **WHEN** the code's behaviour for a modelled step changes but the model does not
- **THEN** the conformance replay fails, naming the trace and step

#### Scenario: Bounds and assumptions are visible
- **WHEN** a reader looks at any proof, model or conformance replay
- **THEN** the input or interleaving bounds and assumptions it covers are written next to it

#### Scenario: A guarantee without a check
- **WHEN** a list guarantee has no proof, model, conformance test, or stated reason that none applies
- **THEN** `just formal-coverage` fails and names the guarantee
