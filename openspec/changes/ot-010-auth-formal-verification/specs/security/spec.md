## ADDED Requirements

### Requirement: Security properties are written down and each has a check

The project SHALL keep a threat model that names the attackers it defends
against:

- a Visitor on the open internet
- a Member acting against other Members
- someone holding a stolen session or refresh token
- someone with control of the network between a client and the edge
- someone holding a copy of the database

It SHALL number each security property that auth relies on. Every property
SHALL name at least one check that covers it (a proof, a model, a test or a
gate) or be marked as an accepted gap with a reason. The gate SHALL fail if a
property names no check and isn't marked as a gap, or names a check that
doesn't exist.

#### Scenario: A property with no check fails the gate

- **WHEN** a property is added to the catalogue without naming a check or marking it as an accepted gap
- **THEN** the full gate and CI fail, naming the property

#### Scenario: A renamed or deleted check is noticed

- **WHEN** a check named by a property is renamed or removed
- **THEN** the full gate and CI fail, naming the property and the missing check

#### Scenario: Gaps are visible

- **WHEN** a reader opens the threat model
- **THEN** every accepted gap is listed with its reason, separately from covered properties

### Requirement: Every route is either public by name or guarded

Every API route SHALL either be on an explicit list of public routes or
require a valid session. A route that acts for an Admin or Super Admin SHALL
enforce that role before the handler runs, in the route definition, not only
in the handler. Adding a route that is neither listed as public nor guarded
SHALL fail the gate.

#### Scenario: A guarded route refuses a Visitor

- **WHEN** a Visitor calls any route that isn't on the public list, with no session or an invalid one
- **THEN** the response is 401, or 403 `csrf_invalid` for a state-changing request (the cross-site check runs first and also needs a session), and the handler does nothing

#### Scenario: An admin route refuses a Member

- **WHEN** a Member with a valid session calls any admin route
- **THEN** the response is 403 and the handler does nothing

#### Scenario: An unguarded new route fails the gate

- **WHEN** a route is added that is neither on the public list nor guarded
- **THEN** the full gate and CI fail, naming the route

### Requirement: A Member can reach only their own account

Every route that reads or changes a Member's account, profile, sessions or
exported data SHALL act on the account named by the caller's session. No
request parameter or body field SHALL be able to name another account. Only
Admins and Super Admins SHALL see other accounts, through the admin routes.

#### Scenario: A Member cannot name another account

- **WHEN** a Member sends a request to any account route carrying another account's id, email or token in the path, query or body
- **THEN** the request acts only on the Member's own account, or is refused

#### Scenario: Exported data is the caller's own

- **WHEN** a Member exports their data
- **THEN** the export contains no data belonging to another account

### Requirement: Code-entry lockout holds for every input

For email-verification and password-reset codes, the system SHALL lock a code
once it reaches 5 failed attempts. A locked code SHALL stay locked however many
more failed attempts are recorded. The lock window SHALL be 15 minutes,
doubled for each earlier lockout, and never more than 24 hours. A longer
lockout history SHALL never give a shorter window. While a code is locked and
inside its lock window, the system SHALL refuse to issue a replacement and
SHALL report how long is left. Outside the lock, the system SHALL refuse a
replacement within 5 minutes of the last send. Every reported wait SHALL be a
whole number of seconds that is never negative. These rules SHALL hold for
every attempt count, every lockout count and every pair of send and request
times, not only the ones a test happens to try.

#### Scenario: A locked code cannot be replaced inside its window

- **WHEN** a Visitor asks for a new code while the current one is locked and its lock window has not elapsed
- **THEN** no new code is issued and the response gives the seconds remaining, which are greater than zero

#### Scenario: The lock window stays within bounds and only grows

- **WHEN** a code is locked after any number of earlier lockouts, including a negative or very large stored count
- **THEN** its lock window is at least 15 minutes and at most 24 hours
- **AND** a code with more earlier lockouts never gets a shorter window than one with fewer

#### Scenario: Extra failed attempts never unlock a code

- **WHEN** failed attempts keep being recorded against a code that is already at the cap, up to the largest count the system can store
- **THEN** the code still reads as locked

#### Scenario: The resend cooldown is respected

- **WHEN** a Visitor asks for a new code less than 5 minutes after the last one was sent, and the code is not locked
- **THEN** no new code is issued and the response gives the seconds remaining

#### Scenario: A replacement is allowed once every wait has passed

- **WHEN** a Visitor asks for a new code after the lock window (if any) and the resend cooldown have both elapsed
- **THEN** a new code is issued

### Requirement: One-time codes are accepted only while live

The system SHALL accept an email-verification or password-reset code only if it
has not expired and is not locked. The system SHALL accept a magic link only
before its own expiry.

#### Scenario: An expired code is refused

- **WHEN** a Visitor enters a code at or after its expiry time, whatever its attempt count
- **THEN** the code is refused

#### Scenario: A locked code is refused even if it is correct

- **WHEN** a Visitor enters the correct code after it has been locked, before it expires
- **THEN** the code is refused

### Requirement: A revoked refresh token is never accepted

The system SHALL treat a refresh token as active only if it has not been
revoked and has not expired. A token that is both revoked and expired SHALL be
reported as revoked, so that replaying a revoked token is always recognised as
possible theft. Revoking a token SHALL be permanent, and revoking it again
SHALL keep the original revocation time.

#### Scenario: A revoked token is refused at any time

- **WHEN** a Member's client presents a refresh token that was revoked, at any time before or after its expiry
- **THEN** the token is refused and reported as revoked, not as expired

#### Scenario: Revoking twice keeps the first revocation time

- **WHEN** an already-revoked token is revoked again
- **THEN** its revocation time does not change

### Requirement: Role management never escalates privilege

A Member SHALL NOT be able to manage any account's role. An Admin SHALL be able
to manage only Members. Only a Super Admin SHALL be able to manage Admins and
Super Admins. A stored role SHALL read back as the same role, and any stored
value that is not a known role SHALL be rejected rather than given a default
role.

#### Scenario: An Admin cannot manage another Admin or a Super Admin

- **WHEN** an Admin tries to change the role of an Admin or a Super Admin
- **THEN** the change is refused

#### Scenario: A Member cannot manage anyone

- **WHEN** a Member tries to change any account's role, including their own
- **THEN** the change is refused

#### Scenario: An unknown stored role is rejected

- **WHEN** an account's stored role is not exactly one of the known roles
- **THEN** it is rejected rather than read as Member or any other role

### Requirement: A deleted account stays deleted

Once an account is deleted (anonymised), no action SHALL make it active or
pending deletion again. Scheduling deletion again SHALL NOT move an existing
deletion date. A pending deletion SHALL be ready to purge only once its
scheduled time has passed. The stored deletion state SHALL read back as the
same state, and a stored record that is both deleted and scheduled SHALL read
as deleted.

#### Scenario: No action revives a deleted account

- **WHEN** a deleted account is asked to schedule deletion, cancel deletion or be anonymised again
- **THEN** each request is refused and the account stays deleted with its original deletion time

#### Scenario: Rescheduling keeps the original date

- **WHEN** a Member who already has a deletion scheduled asks to delete their account again
- **THEN** the scheduled date does not change

#### Scenario: A pending deletion is not purged early

- **WHEN** the purge job looks at an account whose scheduled deletion time is still in the future
- **THEN** the account is not purged

### Requirement: Email addresses normalise to one identity

The system SHALL treat two addresses that differ only in letter case or
surrounding whitespace as the same account. Normalising an address that is
already normalised SHALL leave it unchanged.

#### Scenario: Case and whitespace variants reach the same account

- **WHEN** a Visitor signs in as ` User@Example.COM` and the account was created as `user@example.com`
- **THEN** both name the same account

#### Scenario: Normalising twice changes nothing

- **WHEN** an already-normalised address is normalised again
- **THEN** the result is identical

### Requirement: Concurrent requests cannot get around auth limits

The code-entry attempt cap SHALL hold under any interleaving of concurrent
requests: across all concurrent guesses at one code, no more than 5 SHALL be
compared before the code locks. A single refresh token SHALL yield at most one
new session, however many requests present it at once. A guess that races
with a successful verification, a resend or expiry cleanup SHALL get an
expired or no-code response, never a misleading "too many attempts".

#### Scenario: A burst of guesses gets at most five comparisons

- **WHEN** 50 wrong guesses at one fresh code arrive at the same time
- **THEN** at most 5 are compared, the rest are refused as locked, and the code is locked

#### Scenario: One refresh token, one new session

- **WHEN** the same refresh token is presented by several requests at the same time
- **THEN** at most one of them receives a new session, and any later use of that token is treated as reuse

#### Scenario: A guess racing a successful verification

- **WHEN** a guess is processed after another request has already verified and removed the code
- **THEN** the guess gets the expired or no-code response

### Requirement: The sign-in protocols resist an active network attacker

Against an attacker who can read, replay, drop and change any message that
isn't protected by TLS, and who can hold their own account, the following
SHALL hold:

- A session or refresh token SHALL be obtainable only by the Member who signed
  in.
- A Google sign-in SHALL complete only in the browser that started it, so an
  attacker cannot sign a victim into the attacker's account (login CSRF).
- A cross-site page SHALL NOT be able to make a signed-in browser perform a
  state-changing request.
- A refresh token stolen and used after its owner has refreshed SHALL end all
  of that Member's sessions.

#### Scenario: A replayed sign-in message gains nothing

- **WHEN** an attacker replays a captured sign-in, refresh or OAuth callback message
- **THEN** they obtain no session for the victim

#### Scenario: Login CSRF through Google sign-in fails

- **WHEN** an attacker starts Google sign-in with their own account and gets a victim's browser to finish it
- **THEN** the callback is refused and the victim is not signed in as the attacker

#### Scenario: A cross-site request is refused

- **WHEN** a page on another site makes a signed-in Member's browser send a state-changing request
- **THEN** the request is refused before the handler runs

#### Scenario: Stolen refresh token reuse ends every session

- **WHEN** an attacker uses a stolen refresh token after its owner has already refreshed with it
- **THEN** every session of that Member is revoked

### Requirement: Auth guarantees are machine-checked on every change

Each guarantee in this specification SHALL be checked by the strongest method
that fits it, and each guarantee SHALL be checked over every input or
interleaving within stated bounds, not only by examples:

- a proof over every input for the pure policy rules
- a model checked over every interleaving for concurrency
- a symbolic proof against the stated attacker for the protocols

Wherever the check is a model rather than the code itself, conformance tests
SHALL tie the model to the running code. These checks SHALL run in the full
local gate and in CI. A change that breaks a guarantee SHALL fail the build
with a concrete counterexample or attack trace. Each check SHALL state its
bounds and its assumptions next to it.

#### Scenario: Breaking a guarantee fails the gate

- **WHEN** a change makes any guarantee above false for some input or interleaving within its bounds
- **THEN** the full gate and CI fail, naming the guarantee and a counterexample

#### Scenario: A model that drifts from the code fails the gate

- **WHEN** the code's behaviour for a modelled step changes but the model doesn't
- **THEN** a conformance test fails, naming the step

#### Scenario: Bounds and assumptions are visible

- **WHEN** a reader looks at any proof or model
- **THEN** the input bounds and attacker assumptions it covers are written next to it
