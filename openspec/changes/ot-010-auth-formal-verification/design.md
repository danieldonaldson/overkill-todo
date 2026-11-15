# Design

## Context

See proposal.md for the motivation. What the code looks like today:

- **Routes** (`infrastructure/api/routes.rs`). `api_routes()` groups routes
  and guards each group through the `Guarded` trait (`require_auth`,
  `require_verified_email`, `require_fresh_session`, `require_csrf`). The
  public groups are `public_auth`, `verification`, `oauth` and `health`. The
  admin group is guarded only by `require_auth`, plus an `AdminUser` or
  `SuperAdminUser` extractor on each handler. So a new admin handler that
  forgets its extractor is open to any verified Member.
- **Role changes** (`application/use_cases/admin/manage_users.rs:83`). These
  already check `can_manage` against both the target's current role and the
  new role.
- **Refresh** (`application/use_cases/auth/refresh_token.rs`). A revoked token
  presented again revokes every session of that user, and a fingerprint
  mismatch is refused. Whether two concurrent refreshes with one token can
  both succeed is not established.
- **Cookies** (`infrastructure/api/cookies.rs`). Session cookies are
  `HttpOnly; SameSite=Lax`, the refresh cookie is scoped to `/api/v1/auth`,
  and the CSRF cookie is readable for double-submit.
- **Domain.** The auth policy is in `backend/src/domain`. Its only external
  dependencies are chrono, uuid and thiserror, but it is compiled in the
  single `overkill-todo` crate, alongside sqlx, aws-lc-rs, tokio and axum.
  Time-dependent methods read `Utc::now()` themselves. About 35 call sites
  outside the domain use them, and there is no clock port.
- **The counter wrap.** `increment_attempts` on both code entities is
  `+= 1` on an `i32`. `[profile.release]` doesn't enable `overflow-checks`, so
  `i32::MAX + 1` wraps to `i32::MIN` and `is_locked()` reads false.
- **Dev tools** are pinned in `.mise.toml`. The gate is `just ci`, and CI has
  a paths filter (`changes.backend`).

## Goals / Non-Goals

**Goals:**

- Each guarantee is checked by the cheapest method that covers it fully. If a
  test can cover a property for every input, it doesn't get a model.
- Every model is tied to the code by conformance tests, and every check is
  reachable from the threat model.
- Each tool is pinned, runs through one `just` recipe, and is part of `just
ci` and CI.

**Non-Goals:**

- Refinement proofs from the models down to the code.
- A clock port (see D3).
- Replacing existing tests. They stay, and the catalogue points to them.

## Hypothesis

**Slice 1 (the catalogue).** We believe most properties are already partly
covered by integration tests. We expect the real gaps to be concurrency (the
refresh-rotation race in particular), the protocol-level properties, and
route-level admin enforcement. Slice 1 tests this by building the catalogue
against the existing tests. We'll know from the count and kind of properties
it marks as gaps, and slices 2+ get re-ordered by what it finds.

_What slice 1 showed._ The catalogue has 32 properties. 14 are covered as
stated, 1 is an accepted gap by design (P-03, login throttling at the edge),
and 17 have gaps. The hypothesis was partly wrong:

- **Admin enforcement: confirmed, and worse than expected.** The admin role
  was enforced only by each handler's extractor. Fixing it at the route
  layer, the first attempt brought in a regression: a just-promoted Member got
  a 403 the client wouldn't retry. Code review caught it, and a test now pins
  the guard order.
- **Concurrency: mostly wrong.** Both races already have a real-database
  concurrency test (`concurrent_rotate_lets_exactly_one_winner`,
  `register_attempt_caps_under_concurrency`). The refresh double-issue race
  the Open Questions worried about is tested, though not for every
  interleaving. Quint is still worth doing, but it isn't urgent.
- **Protocol: confirmed.** CSRF, Google sign-in state, and login CSRF are
  tested message by message only.
- **Surprise: the cheapest real gaps need no formal tools.** Five properties
  lack an ordinary integration test:
  - P-04: a session issued before a password reset is refused. Only checked
    with mocks today.
  - P-09: a Member can't reach another account. Only checked with one account
    in the database.
  - P-17: a refresh token used after logout is refused.
  - P-21: the CSRF-exempt routes can't be driven cross-site. Strength: none.
  - P-27: a database copy holds no plaintext credentials.
- **Surprise: the pinned toolchain isn't enforced locally.** A system Rust on
  PATH ahead of rustup silently overrides it, even inside `just`.
  `just toolchain-check` checks only that the two pin files agree, not what
  actually runs. It's out of scope here, but worth its own fix.

So the order changes. Slice 2 closes the five plain-test gaps, because each is
a real, unverified security claim, and closing it costs a day or less.
Authorisation by construction was mostly delivered in slice 1, and its
remainder (P-09) moves into slice 2. Kani becomes slice 3, then Quint,
Tamarin and fuzzing.

**Slice 3 (Kani).** We believe Kani can build the domain and prove the
`AttemptLockout` properties in under about 5 minutes, with bounded symbolic
timestamps. The first task of that slice tests this with a trivial harness.

## Decisions

### D1: The threat model is a Markdown file the gate can parse

It lives at `docs/security/threat-model.md`, linked from README and
ARCHITECTURE.md. Each property is a heading `### P-<nn>: <statement>`
followed by a `Checks:` list and an optional `Gap:` line. Each check entry is
one of:

- a test path, `backend/tests/<file>.rs::<fn>` or `<src path>::<mod>::<fn>`
- a Kani harness, using the same path form
- a model property, `models/<file>.qnt::<invariant>` or
  `models/<file>.spthy::<lemma>`
- a fuzz target, `fuzz/fuzz_targets/<name>.rs`
- a `just` recipe

`scripts/threat-model-check.sh` (`just threat-model-check`, in `ci`) fails if
a property has neither a check nor a `Gap:` line, or if any check doesn't
resolve. A path or recipe must exist, and a function, invariant or lemma name
must appear in that file. Grep-level resolution is enough: the aim is to catch
renames, not to prove the tests are meaningful.

We considered and rejected:

- **Putting properties in the OpenSpec spec**, because a spec describes
  behaviour, not where it is checked.
- **A YAML catalogue**, because it is harder to read as a document, and the
  document is the point.

### D2: Route guards are checked from the OpenAPI description, admin roles are enforced at the route level

`Guarded` gains `require_admin()` and `require_super_admin()` (infrastructure
layer). The admin group uses them. The `AdminUser` and `SuperAdminUser`
extractors stay on each handler as a second check.

A new integration test, `backend/tests/route_guards_integration.rs`, walks
every operation in the committed `openapi.json`, which `api_routes()` already
guarantees is complete. It checks three things:

- every operation not on a `PUBLIC_ROUTES` list in the test returns 401
  without a session
- every `/admin` operation returns 403 for a Member session
- every entry on `PUBLIC_ROUTES` still exists

Because the route set comes from the description, a new route is covered
without anyone remembering to add it.

_Decided in slice 1:_

- The test builds the description live from `openapi::openapi()`, not from
  the committed `openapi.json`. They're the same thing (`api-types-check`
  enforces that), and the live one can't be stale.
- The test checks whether a route can be **reached**, not which layer refuses
  it. Removing `require_auth()` from the profile group doesn't open those
  routes: `require_fresh_session` fails closed, and every profile handler
  extracts `AuthUser`. So that scratch check passes, correctly. What the test
  does catch, as scratch-checked: an open route not on the public list, a
  stale public entry, and an admin group with its route guards removed.
- A state-changing request with no session gets 403 `csrf_invalid`, not 401,
  because the CSRF check runs first and itself needs a valid session. The
  test accepts either, and the spec scenario says so.
- The admin routes split into two groups: `require_admin()` for
  `list_users`, and `require_super_admin()` for `set_user_role`, which was
  already Super Admin only through its extractor. Both guards are the
  existing extractors applied as route layers (`from_extractor`), so the role
  rule has one definition.
- The role guard runs _after_ the fresh-session check (code review). A Member
  just promoted to Admin still holds a token naming the old role. It must get
  fresh-session's 401, which makes the client refresh and retry, not a 403
  from the stale role, which the client doesn't retry. Members pay one
  database read on admin routes, which are low-traffic.
- The guard chains are the public `admin_guards` / `super_admin_guards`,
  generic over router state. The test wraps them around a probe handler with
  no extractor, which is the only way to show the route layer enforces the
  role by itself: on the real routes, the handler extractor returns the same 403.
- Admin routes live under `/api/v1/admin` by convention. The Member walk
  checks only that prefix.

We considered:

- **A Rust type-level design**, where handlers can only be mounted inside a
  guard. Stronger, but it fights utoipa's `routes!`.
- **Inspecting the router**. Axum doesn't expose its layers.

### D3: Policy takes `now: DateTime<Utc>`; callers pass `Utc::now()`

Model checking needs time as an input. The time-dependent domain methods take
`now`, and application callers pass `Utc::now()` at the call site. The change
is made one slice at a time, for the types that slice proves. A `Clock` port
was rejected: it would add a dependency-injection seam across every use case
for no gain over a plain argument.

### D4: Kani for domain policy, with harnesses beside the code

Kani checks unmodified Rust, catches panics and overflow without being asked,
and its harnesses look like tests. We rejected:

- **Verus**, which can't handle chrono, so we'd be verifying a mirror that
  could drift.
- **Aeneas**, which is the most effort. It's worth revisiting for unbounded
  lockout proofs.
- **More proptest**, which is still sampling.

Each domain file with guarantees gets a `#[cfg(kani)] mod verification`
beside its `#[cfg(test)] mod tests`. `cfg(kani)` is added to `check-cfg` in
`backend/Cargo.toml`. Kani is pinned in `.mise.toml` as
`"cargo:kani-verifier"`, then `cargo kani setup`. It brings its own nightly,
which doesn't affect the pinned toolchain.

The recipe is `just verify`
(`cd backend && SQLX_OFFLINE=true cargo kani`), and CI gets a `verify` job
under `changes.backend`.

**Fallback.** If Kani can't build the crate, or the build takes more than
about 5 minutes, extract `src/domain` into a workspace crate
`backend/crates/domain` (chrono, uuid, thiserror only) and verify that. This
would also enforce "domain has no I/O crates" mechanically.

### D5: Symbolic time is bounded epoch seconds

Harnesses build timestamps from `kani::any::<i64>()` seconds, limited by
`kani::assume` to 2000-01-01..2200-01-01. If chrono's arithmetic is
intractable, extract an `i64`-seconds kernel, such as
`reissue_at(attempts, lockouts, sent_secs, now_secs)`, that the chrono method
calls, and prove the kernel instead.

### D6: The counter wrap is fixed with `saturating_add`

Both `increment_attempts` methods become `saturating_add(1)`. We rejected
turning on `overflow-checks` in release, because a panic in a request handler
is a 500, which is a different bug.

### D7: Email is proven only for short ASCII

Unicode lowercasing is intractable for the solver. Harnesses bound input to 8
ASCII bytes. The existing proptests keep covering Unicode.

### D8: Quint for concurrency, with ITF traces as conformance tests

`models/attempt_claim.qnt` and `models/refresh_rotation.qnt` model each
request as a sequence of atomic database steps. Each step is one SQL
statement, or one transaction at its isolation level. The models check the
invariants from the spec's concurrency requirement: at most 5 comparisons, at
most one live successor per refresh token, and racing guesses get the
no-code response.

Two recipes run them:

- `just models` runs `quint verify`, bounded, in `ci`. Quint is pinned in
  `.mise.toml` as an npm tool.
- `quint run --out-itf` exports traces. A `#[sqlx::test]` conformance suite
  replays them against the real repositories with real concurrent
  connections, and asserts that the database reaches the state the model
  predicts.

Replaying the traces is what keeps the model honest.

We chose Quint over TLA+ for its readable syntax, the same model checkers
underneath (Apalache/TLC), and ITF trace export, which Rust can read with the
`itf` crate.

### D9: Tamarin for the protocols, with lemmas linked to tests

`models/auth_protocols.spthy` models password sign-in, refresh with rotation
and reuse detection, Google sign-in (state cookie and device fingerprint), and
the CSRF double-submit. It uses Tamarin's Dolev-Yao attacker, plus an attacker
who has their own account. TLS is modelled as a secure channel, and cookies as
browser state bound to an origin. The lemmas are the spec's protocol
scenarios.

Tamarin runs through `just protocols` from a Docker image pinned by digest,
since mise has no Tamarin package. It is in `ci`.

A symbolic model can't replay against the code, so conformance here means
something else: each model rule that encodes a check the code must make
(state matches the cookie, CSRF header equals the cookie, refresh token not
revoked) names the integration test that shows the code makes it. The
threat-model check (D1) resolves those links.

We chose Tamarin over ProVerif because refresh rotation with revocation is
stateful, and Tamarin handles mutable state natively. ProVerif is the
fallback if proofs don't terminate.

### D10: cargo-fuzz targets, time-boxed in the gate

The targets are:

- token verification (session, signup and reset)
- OAuth state decoding
- cookie and header parsing
- the full router via tower `oneshot`, with in-memory fakes for the outgoing
  ports, so it needs no database

`just fuzz-smoke` runs each target for 60 seconds in `ci`, from a corpus
committed to the repo. Longer runs are manual. cargo-fuzz needs nightly, so
`just fuzz-smoke` uses `cargo +nightly` with the nightly pinned in the recipe.

## Risks / Trade-offs

- [Kani lags the pinned toolchain, or chokes on a dependency] → The D4
  fallback crate.
- [The gate gets slow: Kani, Quint, Tamarin and fuzzing could add several
  minutes] → Each runs as its own CI job under the paths filter. If one
  exceeds about 10 minutes, it runs only when its inputs change (domain,
  models, or `routes.rs` and `cookies.rs`).
- [The models drift from the code] → Quint traces replay against the real
  database (D8). Tamarin rules name their tests (D9). The catalogue gate
  catches broken links (D1).
- [Bounds hide a bug outside them] → Bounds and assumptions are written next
  to every check (spec), and the catalogue lists accepted gaps openly.
- [False confidence that auth is "proven secure"] → The threat model states
  its attackers and its gaps, and README says the same next to `just verify`.
- [The proofs find defects that change behaviour, for example concurrent
  refresh] → The fix ships in the slice that finds it. Its behaviour is
  already defined by the spec, and the frontend refresh flow is retested
  (performance spec "each refresh succeeds…").

## Open Questions

- Router fuzzing (D10) needs in-memory fakes for every outgoing port. If some
  don't exist yet, building them is part of the fuzz slice. This changes the
  size of that slice, not its approach.
- Whether the concurrent-refresh model shows a double-issue race in some
  interleaving the existing test doesn't hit. Slice 1 found that one
  concurrent run already passes. If the model finds a race, the fix (a
  conditional revoke-and-insert) is designed in that slice.
