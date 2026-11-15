## ADDED Requirements

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
