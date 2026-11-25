# Design

## Context

See proposal.md for why. What shapes the approach:

- **The CSRF token doesn't help.** It is bound to the session's account, but the
  client reads its cookie when sending, so a stale tab sends the new account's.
- **The client resends.** `HttpClient` repeats a request after a 401 or
  `csrf_invalid`, with whatever cookies are current by then.
- **The tab's account** is cached per tab in `sessionStorage` (`auth/session.ts`),
  so it stays A when B signs in from another tab.
- **OT-013** checks a marker in `localStorage` before each list save
  (`ensureSameAccount`). It covers lists only and can't see a later switch.

## Goals / Non-Goals

**Goals:**
- No authenticated change can be added later without the check.
- Compatible in both directions: old client with new server, new with old.

**Non-Goals:**
- Covering reads, or detecting a switch before anything is sent.

## Hypothesis

We believe taking the account ID once, when `request()` is first called, and
carrying it through every resend is enough. Slice 1 tests it with a Playwright
integration flow on the real backend: tab 1 (as A) has a save held before it is
sent, tab 2 signs in as B, the save is released. It holds if the save is refused
and B has no list, and if the same test fails when the ID is read on each
attempt instead. A unit test does the same for the 401 and `csrf_invalid`
resends.

## Decisions

### D1. An `X-Account-Id` header (api)

The client names the account in `X-Account-Id`, an account UUID. It is compared
with `AuthUser.user_id`. Anything that isn't exactly that UUID, including a
repeated or empty header, is a mismatch: a value that can't be read must not
pass as "not given". Added to the allowed CORS headers.

- **Rejected: an ID in each body or path.** It would change every DTO, and
  `DELETE` has no body.
- **Rejected: per-tab sessions.** Cookies are shared by design.
- **Rejected: remembering the CSRF token per tab.** It changes on each refresh,
  so a correct tab would be refused.

### D2. 412 `account_changed`, not 409 (api)

The header is a precondition. 409 is already described with other bodies (`stale`
needs `current`), so a second meaning there would break the description and make
clients tell two conflicts apart. No existing route answers 412. The body is the
standard `ErrorResponse` with a fixed message naming no account.

### D3. Part of `require_auth()` (api)

`auth_middleware` compares after it has the principal, for `POST`, `PUT`, `PATCH`
and `DELETE`, and the guard documents the 412 on those. Every signed-in route
already needs `require_auth()`, so none can miss it, and it runs before
fresh-session, role and handler checks. `logout` and `refresh` are public routes
and stay outside.

A guard test walks the API description: each changing operation that requires
the session cookie documents 412 and answers it.

- **Rejected: a separate `Guarded` method.** A new group could forget it.
- **Rejected: in the CSRF layer.** It also covers public routes, and webhooks sit
  below it.

### D4. The header is described, and the fuzzer sends the right one (api, apifuzz)

The guard adds it as an optional header parameter. Fuzzing would otherwise send
random UUIDs and get only 412s, so `hooks.py` sends the fuzz account's ID on most
requests and leaves it out on some. A custom check sends another account's ID and
expects 412.

### D5. The client takes the ID once (web, `lib/api/client.ts`)

On the first attempt of a changing request, before `ensureValidToken`, it reads
`getSessionUser()?.id`, and passes it to its own resends. No cached account means
no header. The ID is read when the request starts; list saves waiting in the
OT-013 queue start later, so the engine's reset on an account change (its epoch)
still drops those.

### D6. One reaction, in the session store (web)

`HttpClient` throws `AccountChangedError`, a `SessionEndedError`, so the lists
engine already treats it as final: no repeat and no message of its own. The
session store registers one handler with the client. It clears the lists at once
(`lists.switchAccount()`), drops the cached account, runs the `/auth/me` check,
shows the toast once per switch, and lets role-dependent pages re-evaluate. Panels
get the error through their existing handling, and must not change the account
record on failure, as today.

Clearing comes before the check, so nothing of A's stays if `/auth/me` fails.

### D7. OT-013's marker stays until the check is deployed (web)

The `localStorage` marker also clears an idle stale tab promptly, through the
`storage` event. Only its wait before each save (`ensureSameAccount`) is removed,
in the final group, once the server check is deployed.

## Risks / Trade-offs

- [The tab's cached account is itself wrong, e.g. B signs in in the same tab and
  acts before `/auth/me` answers] → the change is refused and the account
  reloaded; nothing is saved wrongly. The sign-in flow clears the cache; slice 1
  tests a change right after signing in.
- [A bug makes the client send a stale ID and blocks every change] → a counter of
  412 `account_changed` by route template, and an alert on a sustained rate.
- [Old servers ignore the header] → during a rollback the OT-013 check still
  covers lists.

## Migration Plan

No schema change. Server first (it accepts requests without the header), then
web. Roll back either one alone.
