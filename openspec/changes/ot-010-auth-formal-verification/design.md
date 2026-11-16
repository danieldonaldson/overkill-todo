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
then Tamarin. (Fuzzing later moved to OT-011, D10.)

_What slice 2 showed._ All five properties hold in the code. No defect
needed fixing beyond a logout that silently dropped revocation errors (it
now logs them). The gaps were in the evidence, and twice in the catalogue's
own claims:

- **The catalogue had two errors.** P-04 was already tested end to end
  (`password_reset_flow_succeeds`), but the test's name hid it. P-27 claimed
  OAuth secrets are encrypted, but nothing calls the cipher, because no OAuth
  secret is stored. Lesson: reading a test's name isn't enough. Slice 1's
  "read each linked test" step needs to look at the assertions too.
- **First-draft tests that couldn't fail.** Code review found that the
  first versions of the P-27, P-21 and P-09 tests had checks that couldn't
  fail. The database dump ran after the code rows were deleted. A body used
  the wrong field name, so it was refused for the wrong reason. Set-password
  was called where it would be refused on either account. Each now has a
  scratch-checked failure mode. Lesson: every new check gets a deliberate
  break before it's trusted, which the formal slices do anyway (a proof that
  fails first).
- **Accepted gaps, now explicit.** `/auth/me` serves a pre-reset access token
  until it expires (P-04). Decided after the slice: `/auth/me` and Google unlink
  now run the fresh-session check too, closing the gap. Logout can't guarantee
  revocation when the database fails (P-17). Logout CSRF depends on browser
  `SameSite` handling (P-21).
- **OT-011 (API fuzzing) overlaps slice 6.** It was proposed while this
  slice ran. Decided: slice 6 is dropped, and P-31 and P-32 link to OT-011's
  checks once it lands (D10).

**Slice 3 (Kani).** We believe Kani can build the domain and prove the
`AttemptLockout` properties in under about 5 minutes, with bounded symbolic
timestamps. The first task of that slice tests this with a trivial harness.

_What slice 3 showed._

- **Kani builds the crate in place.** The first `cargo kani` took about 51 s
  wall (4 m 22 s CPU) cold, and `just verify` takes about 13 s warm for all 9
  harnesses. The domain-crate fallback (D4, task 3.2) wasn't needed, and the
  user moved that extraction to a separate change.
- **Proving through chrono doesn't finish; the D5 fallback was needed.** The
  first reissue harness over `DateTime` ran for more than 12 minutes in CBMC
  and was stopped. The policy now lives in integer `const fn`s over
  microseconds (`reissue_at` and friends), which the `AttemptLockout` methods
  call after `timestamp_micros()`. Each harness solves in under a second
  (0.08–0.86 s). Microseconds rather than D5's seconds keep the integer policy
  exactly equivalent to the old chrono arithmetic, so nothing had to be
  rounded away to make it provable.
- **Two real defects found.**
  - **Lock lifted up to a second early.** Wait times were rounded down to
    whole seconds, so a lock or cooldown with under a second left reported
    zero and a locked code could be reissued up to a second early. The
    counterexample was a 24 h lock with 0.93 s left reporting `Allowed`. Waits
    now round up (`whole_seconds_until`), with a regression unit test.
  - **The domain counter wrapped.** `increment_attempts` overflowed at
    `i32::MAX`, and the proof failed with "attempt to add with overflow". It
    was made to saturate. Code review then showed nothing in production calls
    it: the real counter is the SQL attempt claim, capped at 5. After the
    slice, the method was deleted, along with its proof and tests, so there's
    only one counter. P-13 links the concurrency test that exercises the SQL
    cap.
- **Proofs can check a function against itself.** The reissue proofs take
  the lock window from the function under test, so they can't catch a wrong
  schedule. A separate proof states the schedule with literal numbers (15, 30
  … 960, 1440), and a scratch break with no doubling fails only that proof.
  Lesson: at least one proof per rule must state the spec independently of
  the code.
- **The proofs cover the decision, not the system.** Review found a
  pre-existing race. The issuing paths decide with `reissue`, then upsert the
  new code with `attempts = 0` and no guard, so a failed guess that locks the
  code in between has its lock overwritten (P-11 gap). That is concurrency in
  the database, and slice 4 models it and fixes it.

**Slice 4 (Quint).** We believe the concurrency properties can be modelled
statement by statement, and that the riskiest part is tying the model to the
code: whether ITF traces from Quint can be replayed against the real
repositories, on separate connections, cheaply enough to run on every change.
The slice fixes the resend race first, with a test that fails before the fix.

_What slice 4 showed._

- **The race was real and easy to hit.** The new race test fires a 5th wrong
  guess and an issuing request at once, 40 rounds per path, over the real
  router. Before the fix, 37, 38 and 37 of 40 rounds (resend, re-register,
  forgot-password) overwrote a lock inside its window. With the guarded upsert
  it passed 20 runs in a row (2,400 rounds), and the use-case unit tests for
  the refused write each fail on a scratch edit that ignores the refusal. The
  guard reads the lock windows from a table the domain exports
  (`LOCK_WINDOWS_MINUTES`), with a Kani proof that the table, indexed as the
  SQL indexes it, equals the policy for every `i32`.
- **Quint 0.33.1 with Apalache works here, so `quint verify` is used, not the
  `quint run` fallback.** Apalache (0.62.4, which Quint downloads) needs Java;
  Temurin 21.0.12 is now pinned in `.mise.toml` next to Quint. The trivial
  model verified in about 3 s (simulation 1.4 s). The real models, every
  interleaving up to 8 steps: email and reset codes about 35–40 s each,
  refresh rotation about 110 s. The random search (`quint run`, Rust backend,
  10,000 executions of 30 steps) takes about a second per model. `just models`
  takes about 4 minutes in all. Depth 8 covers the races found here (4 and 5
  steps) with room to spare; deeper bounds grow fast (refresh at 8 is already
  most of the time).
- **Conformance replay worked, and catches SQL drift.** 300 random traces per
  model plus 6 witness traces replay in about 12 s, each step through the
  repository method that issues that statement, one connection per request,
  checking results and stored rows after every step. Removing the upsert's
  guard failed at `email_codes_15.itf.json step 6 issueWrite(1)`, and removing
  rotation's `revoked_at IS NULL` failed at
  `refresh_rotation_103.itf.json step 11 refreshRotate(3, token 0)`. Two
  things were needed beyond the plan: random traces reached a refused write
  only about 1 time in 100, so `just models` adds witness traces (a run whose
  "invariant" is that no write is refused, exported as its counterexample);
  and the suite counts how often each interesting outcome came up and fails
  if one never did, so a change in the generator can't make it vacuous.
- **The model found a second defect, now fixed.** The attempt claim named only
  the user. A guess that read a code which was then legitimately replaced
  claimed a slot on the new code but was compared against the old one, past
  the old code's cap, and could be told "locked" about a code it never saw (a
  5-step counterexample). The claim and its release now name the code id.
  Behaviour change: such a guess now gets the no-code answer.
- **Old-code variants keep the invariants honest.** The `unguarded_upsert`
  and `claim_by_user` modules model the code before this slice, and
  `just models` fails unless both still have counterexamples. Each invariant
  also failed on its own scratch break of the model.
- **Two bounds the model made visible, recorded as gaps rather than fixed.**
  The claim's existence check reads its statement's snapshot, so a guess whose
  claim waits on a concurrent replacement can still get "locked" instead of
  "no code" (message only; P-12). And once an expired refresh token is
  deleted by the cleanup job, replaying it is refused as unknown rather than
  treated as reuse (P-15); it can't yield a session. Whether to keep revoked
  tokens past expiry to close that is a decision for the user.
- **Code review found three more ways round the same guard, all fixed.**
  The first guard re-checked only the lock, so issuing requests that all
  decided just after the cooldown could each replace the code, each bringing
  fresh guesses; the guard is now the whole reissue rule (lock window and
  cooldown), with a `lock_only_guard` variant that must still fail. The
  registration path's cooldown release after a failed email moved the send
  time of whatever code the user had, which also starts the lock window; it
  now names the issued code and skips a locked one. And the model stamped a
  new code's send time at the decision rather than at the write, so the
  replay didn't exercise production's stamping; the model now stamps at the
  write and the replay issues codes through the production constructors. The
  review also caught that a refused resend's follow-up read could turn a
  uniform 200 into a 500 (now best-effort), and that the witness step
  accepted any Quint failure as a witness (now checked). Two findings were
  rejected: comparing with the database's `NOW()` instead of the code's
  app-stamped send time (the app clock is the one time source, D3, and the
  decision the guard backs up uses it), and sharing the guard SQL between
  the two repositories (`query!` needs literal SQL, and both are replayed).
- **The concurrent-refresh open question is answered: no double issue.**
  Within the bounds, no interleaving gives a token two successors.

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

_Decided in slice 3:_ the proven policy is integer `const fn`s over
microseconds since the epoch (D5's fallback, with microseconds rather than
seconds for exactness). "At the cap" has one definition, `at_cap`, shared by
`is_locked` and the integer policy. Waits round up to whole seconds.

Both `increment_attempts` methods become `saturating_add(1)`. We rejected
turning on `overflow-checks` in release, because a panic in a request handler
is a 500, which is a different bug. _Superseded after slice 3:_ the methods
had no production caller and were deleted. The only counter is the SQL
attempt claim.

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

_Decided in slice 4:_

- `just models` runs `scripts/models.sh`, which CI runs too: `quint verify`
  (Apalache, 8 steps) for each model as the code stands, the old-code variants
  that must still fail, `quint run` (30 steps), then the trace export and the
  replay. Java (for Apalache) is pinned in `.mise.toml` with Quint; CI installs
  both through mise.
- Traces are generated on each run, not committed: Quint's output isn't
  byte-stable for a seed, and fresh random traces explore more over time.
  The replay suite is `#[ignore]`d in `just test-integration` and run by
  `just models`, which supplies the traces.
- "Concurrent connections" means each request in a trace has its own
  database session, and the steps run in the trace's order. Each step is one
  statement, so this reproduces exactly the interleaving the model chose.
- Time in the models is whole minutes. A `tick` moves every stored timestamp
  back, so the database's `NOW()` stays the model's `now`, and comparisons
  with `NOW()` are written for a database clock a little ahead of the model's.

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

### D10: Fuzzing moves to OT-011

This change planned cargo-fuzz targets for token parsing and the router.
OT-011 (API fuzzing), proposed while slice 2 ran, fuzzes every described
operation signed out and signed in, and checks for the JSON 400, no 500s and
no leaked internal text. That covers P-31 and P-32 more broadly than this plan
did, so OT-010 dropped its fuzzing slice and the matching spec requirement.
The two properties link to OT-011's checks when it lands.

## Risks / Trade-offs

- [Kani lags the pinned toolchain, or chokes on a dependency] → The D4
  fallback crate.
- [The gate gets slow: Kani, Quint and Tamarin could add several
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

- Whether the concurrent-refresh model shows a double-issue race in some
  interleaving the existing test doesn't hit. Slice 1 found that one
  concurrent run already passes. If the model finds a race, the fix (a
  conditional revoke-and-insert) is designed in that slice.
  _Answered in slice 4:_ no. Within 8 steps (Apalache) and 30 steps (random
  search), no interleaving of 3 refreshes, a sign-out and cleanup gives a
  token two successors.
