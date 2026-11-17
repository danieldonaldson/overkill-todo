# api Specification

## Purpose

Defines the contract the backend HTTP API offers its clients: a published
OpenAPI description that covers every route, describes request, response and
error shapes accurately, and is kept true by the build rather than by memory.

## Requirements

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

#### Scenario: Lockout responses

- **WHEN** a code-entry route refuses a request because of a lockout
- **THEN** the status it returns is described with the body it actually sends (the verification error body for verify-email, the standard error body for verify-reset-code)

#### Scenario: Malformed request

- **WHEN** a client sends a route a malformed body, query string or path parameter
- **THEN** the response is 400 with the standard error body, never a plain-text or 415/422 response
- **AND** the description documents that 400

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

### Requirement: Authentication is described as clients really use it

The API description SHALL state, for every route that needs a signed-in caller,
that the caller is identified by the session cookie. For every state-changing
route that needs a CSRF token, it SHALL state that the token is sent in the CSRF
header. It SHALL NOT describe an authentication method the backend doesn't
accept. It SHALL NOT refer to an authentication scheme it doesn't define.

#### Scenario: Protected route

- **WHEN** a client reads the description of the current-user route
- **THEN** it says that the caller is identified by the session cookie
- **AND** it does not offer bearer-token authentication

#### Scenario: Session token sent as a bearer token

- **WHEN** a client sends a valid session token only in an `Authorization: Bearer` header to the current-user route
- **THEN** it gets the documented 401, as if it had sent no session

#### Scenario: State-changing protected route

- **WHEN** a client reads the description of the password-change route
- **THEN** it says that both the session cookie and the CSRF header are required

#### Scenario: Public route

- **WHEN** a client reads the description of the sign-in or health route
- **THEN** it lists no authentication requirement

#### Scenario: Reference to an undefined scheme

- **WHEN** a route's description refers to an authentication scheme that the description doesn't define
- **THEN** the full gate fails and names the route

### Requirement: The backend's behaviour is fuzzed against its description

The project SHALL have one command that sends every described route many
generated requests, both valid and malformed, and checks each response against
the description. It SHALL run against a backend and database that hold only
data it created. It SHALL run every route twice: once as a Visitor and once
signed in, as a Member for Member routes and as an Admin for admin routes. A run
SHALL fail when any of the following is true, and SHALL print a request that
reproduces each failure:

- a response has a 5xx status;
- a response's status, content type or body isn't what the description documents for that route;
- a malformed request is not refused with 400 and the standard error body;
- a route that needs a session, a role or a CSRF token gives a caller without it anything other than its documented 401 or 403;
- an error body is not the standard or verification error shape, or its message contains internal text (database, panic, stack-trace or library error text).

#### Scenario: Clean run

- **WHEN** the developer runs the fuzz command on a backend that behaves as described
- **THEN** it reports how many requests it sent to each route and exits successfully

#### Scenario: Server error

- **WHEN** some generated input makes a route respond with a 5xx
- **THEN** the run fails and prints a request that reproduces it

#### Scenario: Undocumented response

- **WHEN** a route returns a status, or a body field, that its description doesn't document
- **THEN** the run fails and names the route, the status and the mismatch

#### Scenario: Malformed request not refused properly

- **WHEN** a route gets a body that isn't JSON, a missing required field or an unparseable path parameter, and answers with a plain-text, 415 or 422 response
- **THEN** the run fails

#### Scenario: Visitor on a Member route

- **WHEN** the Visitor pass calls a route that needs a session
- **THEN** each response is the documented 401 with the standard error body, and the run fails if any response returns data

#### Scenario: Member on an Admin route

- **WHEN** the Member pass calls the admin user list or the role change, including for another Member's account
- **THEN** each response is the documented 403, and the run fails if any of them succeeds

#### Scenario: Missing CSRF token

- **WHEN** a signed-in caller sends a state-changing request without a valid CSRF header
- **THEN** each response is the documented 403, and the run fails if any of them succeeds

#### Scenario: Internal text in an error

- **WHEN** an error response's message contains database, panic or library error text
- **THEN** the run fails and shows the message

#### Scenario: The session ends part-way through

- **WHEN** a generated request signs the fuzz Member out, changes their password or deletes their account
- **THEN** the run continues with a new session and doesn't report the following 401 responses as failures

#### Scenario: Developer data is untouched

- **WHEN** the developer runs the fuzz command while their local development database holds accounts
- **THEN** those accounts are unchanged afterwards

#### Scenario: Backend or database doesn't start

- **WHEN** the backend or its database can't be started for a run
- **THEN** the command fails and says so; it never reports success without having sent requests

#### Scenario: Reproducing a failure

- **WHEN** a run fails
- **THEN** running the command again with the seed it printed sends the same requests

### Requirement: Fuzzing runs in the gate

A short fuzz run SHALL be part of the full gate, and part of CI on every push to
`main` and every pull request that changes the backend or the API description.
A failure SHALL fail the gate, so no image is built from a commit that fails
it. A longer run SHALL run every night against `main`. A nightly failure SHALL
be reported where the developer will see it, but SHALL NOT fail other runs.

#### Scenario: Change to the backend

- **WHEN** a commit or pull request changes backend code or the API description
- **THEN** the short fuzz run is among its CI checks and finishes in a few minutes
- **AND** if it fails, the run fails and no image is built from that commit

#### Scenario: Full gate run locally

- **WHEN** the developer runs the full gate
- **THEN** the short fuzz run is part of it, against its own throwaway database

#### Scenario: Pull request that only changes the frontend

- **WHEN** a pull request changes only frontend files
- **THEN** the fuzz run is skipped and reports success

#### Scenario: Short run is repeatable

- **WHEN** the short run is repeated on the same commit
- **THEN** it sends the same requests and gives the same result

#### Scenario: Nightly finding

- **WHEN** the nightly run finds a failure that the short run missed
- **THEN** the developer is notified with the failure and its reproducing request, and other CI runs are unaffected
