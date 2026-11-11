# Spec Delta

## Purpose

Defines the contract the backend HTTP API offers its clients: a published
OpenAPI description that covers every route, describes request, response and
error shapes accurately, and is kept true by the build rather than by memory.

## ADDED Requirements

### Requirement: The API description covers every served route
The project SHALL publish an OpenAPI description of the backend API that
describes every route the backend serves, with its HTTP method, path parameters,
query parameters, request body and responses. A route MAY be left out only if it
is on an explicit exemption list that gives the reason for leaving it out.
Every route in the description SHALL be served by the backend.

#### Scenario: Every served route is described
- **WHEN** a client reads the API description
- **THEN** it finds an entry for each route the backend serves, except the listed exemptions
- **AND** the password-reset, magic-link sign-in and Google-account unlink routes are among them

#### Scenario: A new route without a description fails the gate
- **WHEN** a developer adds a route to the backend without describing it or listing it as exempt
- **THEN** the full gate fails and names the undescribed route

#### Scenario: A described route that is not served fails the gate
- **WHEN** the description contains a path or method that the backend does not serve
- **THEN** the full gate fails and names that path

#### Scenario: An exemption without a reason is rejected
- **WHEN** a developer adds a route to the exemption list without a reason
- **THEN** the full gate fails

### Requirement: Field presence and nullability are described as sent and accepted
For every request body, the description SHALL mark a field as required only if
the backend rejects a request that omits it. For every response body, the
description SHALL mark a field as required only if the backend always includes
it. It SHALL mark a field as nullable only if the backend can send `null` for it.

#### Scenario: Optional request field
- **WHEN** a client reads the description of the sign-in request
- **THEN** the Turnstile token is described as optional
- **AND** a sign-in request that omits it is accepted when bot protection is off

#### Scenario: Response field that is sometimes absent
- **WHEN** a client reads the description of the current-user response
- **THEN** fields that are omitted when empty (pending email, scheduled deletion) are described as optional, not as always present with a `null` value

#### Scenario: Response field that is always present
- **WHEN** a client reads the description of the current-user response
- **THEN** the user's ID, email and verification status are described as required and not nullable

### Requirement: Fields with a fixed set of values list them
When a request or response field only ever holds one of a fixed set of string
values, the description SHALL list those values. It SHALL NOT describe such a
field as a free-form string.

#### Scenario: User role
- **WHEN** a client reads the description of the current-user or admin user-list response
- **THEN** the role field lists exactly `user`, `admin` and `super_admin`

#### Scenario: Linked providers
- **WHEN** a client reads the description of the current-user response
- **THEN** each entry in linked OAuth providers lists the supported provider values

### Requirement: Error responses are described
For each error status a route can return, the description SHALL name the body
shape: either the standard error body (a machine-readable code and a message) or
the structured verification error body (a code plus attempt or cooldown details).
The description SHALL NOT include internal error text, example stack traces or
dev-mode-only messages.

#### Scenario: Standard error body
- **WHEN** a client reads the description of a route that can return 401, 403, 404, 409 or 500
- **THEN** each of those statuses names the standard error body, with `code` and `message` required

#### Scenario: Verification error body
- **WHEN** a client reads the description of the verify-email route
- **THEN** its 400 response names the verification error body, including the optional attempts-remaining and cooldown fields

#### Scenario: Lockout and cooldown responses
- **WHEN** a code-entry or resend route refuses a request because of a lockout or cooldown
- **THEN** the status it returns is described with the verification error body, so a client can read the cooldown from it

### Requirement: The committed description stays current
The API description SHALL be committed to the repository. It SHALL be produced
from the backend's code by one command, without a running backend or database.
The full gate SHALL fail if the committed description differs from what the
backend's code produces.

#### Scenario: Shape change without regenerating
- **WHEN** a developer changes a request or response field in the backend and does not regenerate the description
- **THEN** the full gate fails and says which command regenerates it

#### Scenario: Regenerating is deterministic
- **WHEN** the description is regenerated twice from the same code
- **THEN** the two outputs are byte-for-byte identical

#### Scenario: No database needed
- **WHEN** the description is regenerated on a machine with no database running
- **THEN** it succeeds

### Requirement: Frontend API types come from the description
The frontend's types for API requests, responses and error bodies SHALL be
generated from the committed description. Frontend code SHALL NOT declare its
own copy of an API payload shape. The full gate SHALL fail if the generated types
are stale, or if frontend code does not type-check against them.

#### Scenario: Stale generated types
- **WHEN** the committed description changes but the generated frontend types are not regenerated
- **THEN** the full gate fails and says which command regenerates them

#### Scenario: Incompatible backend change
- **WHEN** a backend field the frontend reads is renamed or removed, and the description and types are regenerated
- **THEN** the frontend type check fails at each place that reads the old field

#### Scenario: Compatible backend change
- **WHEN** a backend response gains a new optional field, and the description and types are regenerated
- **THEN** the frontend type check still passes with no frontend edits

#### Scenario: Frontend-only helpers remain
- **WHEN** frontend code checks whether a role grants admin access
- **THEN** it uses the same helper as before, now typed against the generated role values

### Requirement: The description is not served in production
The backend SHALL serve the API description and its interactive explorer only in
local development. Production builds SHALL NOT serve either. Committing the
description to the repository does not change this.

#### Scenario: Production request for the description
- **WHEN** a Visitor requests the description or explorer URL from a production deployment
- **THEN** the response is not the description (it is a not-found response)

#### Scenario: Local development
- **WHEN** the developer runs the backend locally in dev mode
- **THEN** the description and explorer are served as today
