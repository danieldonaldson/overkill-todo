# OT-011: API fuzzing

## Why

As the developer of overkill-todo, I want the whole HTTP API attacked with
generated, mostly malformed requests on every change, so that I can't ship a
crash, an undocumented response or a leaked internal error without the gate
noticing.

OT-002 made the API description accurate and the gate checks that it covers
every route and stays current. Nothing checks that the backend actually
*behaves* as described. Several rules are kept by convention and code review
alone: malformed input gets the JSON 400 and never a plain-text 400, 415 or 422;
protected routes refuse callers without a session or CSRF token; error bodies
never carry internal text. Property tests cover individual parsers (tokens,
emails, CSRF), not the routes that wire them together. There is already proof
that the description and the server have drifted apart: protected operations
refer to a `bearer_auth` scheme the description never defines, and the backend
doesn't take bearer tokens anyway. It uses a session cookie plus a CSRF header.
The API surface today is small (auth, account and admin), so now is the cheap
time to start, before todo endpoints double it.

## What Changes

- The API description says how clients really authenticate (the session cookie,
  plus the CSRF header on state-changing requests) and doesn't refer to schemes
  it never defines.
- One local command fuzzes every described operation against a throwaway,
  freshly migrated database and the backend, in two passes: signed out and
  signed in as a Member (and an Admin for admin routes). It passes or fails and
  prints a reproducing request for each failure.
- A run fails on any of these: a 5xx; a response whose status, content type or
  body the description doesn't document; a malformed request that isn't refused
  with the JSON 400; a protected route that serves a caller without valid
  credentials; an error body that isn't the standard shape or contains internal
  text.
- A short, repeatable fuzz run is part of `just ci` and of CI whenever the
  backend changes, so a failing commit gets no image. A longer run with fresh
  inputs runs nightly. Nightly failures are reported but don't fail anything
  else.
- Whatever the first runs find is fixed in this change if it is spec drift
  (undocumented statuses, wrong schemas). Real defects get fixed here when
  small, otherwise each becomes its own task or change.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `api`: the description must describe authentication accurately, and the
  backend's behaviour must be checked against the description by fuzzing in the
  gate.

## Impact

- **Backend:** OpenAPI annotations (security scheme). Small fixes to handlers
  or extractors for whatever the fuzzer finds. Regenerated `openapi.json` and
  `schema.d.ts`.
- **New tooling:** a pinned API fuzzer plus a small hooks file (sign-in, extra
  checks), a `justfile` recipe, a CI job and a nightly workflow.
- **No** migrations, no new runtime dependencies, nothing deployed.

## Appetite

About 1 week, in two slices.

## Out of Scope

- Coverage-guided fuzzing of individual Rust functions (cargo-fuzz). It's not
  worth it until the backend has a hand-written parser for rich input.
- Stateful sequences (create, then read, update and delete). Revisit when todo
  CRUD endpoints exist.
- The success paths of email-gated flows (verifying a real code from an email).
  Fuzzing covers their failure paths.
- Fuzzing the real Google OAuth round trip, the deployed edge or production.
- Load and timing. OT-004 covers that.

## Dependencies

- OT-002's committed API description and its generated frontend types.
- The dev-mode test seams that already exist: no-op bot protection and MX
  checks, console email, and the fixed verification code for `test-*` addresses
  that OT-004's seeding uses.
- No external services and no data migrations.

## Ready

- **Riskiest unknown:** noise. Will the first run give a short list of real
  findings, or a flood of failures from validation rules the description doesn't
  express (e.g. password length or email format), with too much to triage in
  the appetite?
- **First slice:** fix the security scheme, then fuzz signed out and signed in
  locally with the built-in checks plus the leak check. Triage the first run into
  "fix the description", "fix the backend" and "tune the fuzzer".
