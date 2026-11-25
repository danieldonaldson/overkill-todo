# Spec Delta

## ADDED Requirements

### Requirement: A change is refused when it was made for another account

A request that changes state and needs a session MAY name the account it was
made for. When it names an account other than the session's, the backend SHALL
refuse it before acting on it, SHALL change nothing, and SHALL answer with an
error code that distinguishes this from every other refusal. The name SHALL only
ever cause a refusal: it never selects the account acted on and never grants
access. A request naming no account SHALL be handled as if this rule didn't
exist.

#### Scenario: Another account

- **WHEN** a request to change something names account A and the session belongs to Member B
- **THEN** it is refused with the account-changed error, and neither account's data changes

#### Scenario: Same account

- **WHEN** a request to change something names the session's own account
- **THEN** it is handled as without the name

#### Scenario: No account named

- **WHEN** a request to change something names no account
- **THEN** it is handled as before this rule existed

#### Scenario: Malformed name

- **WHEN** the name isn't a valid account ID, is empty, or is given more than once
- **THEN** the request is refused with the account-changed error and changes nothing

#### Scenario: Every change is covered

- **WHEN** any route that changes state and needs a session is sent another account's ID, as a Member, Admin or Super Admin
- **THEN** it answers the account-changed error

#### Scenario: No session

- **WHEN** a Visitor sends a change naming any account
- **THEN** the answer is the same 401 as without the name

#### Scenario: Reading

- **WHEN** a request that only reads names another account
- **THEN** it is answered for the session's account

#### Scenario: Nothing is revealed

- **WHEN** a request is refused because it names another account
- **THEN** the answer is identical whether or not the named account exists, and names neither account
