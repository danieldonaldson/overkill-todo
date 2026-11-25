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
