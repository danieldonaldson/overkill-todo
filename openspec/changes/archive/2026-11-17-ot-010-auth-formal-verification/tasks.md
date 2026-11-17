# Tasks

Slices are planned in detail one at a time: the next is added once the
current one ships and design.md Hypothesis records what it showed. After
slice 2, the order (reordered by slice 1's findings) is:

3. domain proofs with Kani (design D3–D7)
4. the Quint concurrency model (D8)
5. the Tamarin protocol model (D9)

Fuzzing moved to OT-011 (design D10).

## 1. Slice 1 (first slice): threat model, coverage gate and route-guard gate

This slice delivers, from specs/security/spec.md:

- Security properties are written down and each has a check: all three
  scenarios.
- Every route is either public by name or guarded: all three scenarios.

There's no production deploy yet, so this slice is done when it's merged with
`just ci` green.

- [x] 1.1 Write `docs/security/threat-model.md` in the D1 format:
  - the five attackers, assets and trust boundaries
  - a numbered `P-<nn>` property for every requirement in the spec delta, plus
    any further properties found while reading `routes.rs`, `cookies.rs`, the
    auth use cases and the middleware

  Verify every requirement in the spec delta maps to at least one property

- [x] 1.2 Fill in each property's `Checks:` from the existing unit and integration tests and `just` recipes, or add a `Gap:` line with a reason. Verify by reading each linked test and confirming it really exercises the property, not just the code nearby
- [x] 1.3 Add `scripts/threat-model-check.sh` and `just threat-model-check`, and add it to `ci` (D1). Verify on a scratch copy that the gate fails when a property has no check and no gap, when a linked test function is renamed, and when a linked file is missing. Then verify it passes on the real file
- [x] 1.4 Add `require_admin()` and `require_super_admin()` to `Guarded` (D2) and apply them to the admin group, keeping the handler extractors. Verify `tests/admin_integration.rs` passes, and a new test shows a Member gets 403 from the route layer even for a handler with no extractor (use a test-only route)
- [x] 1.5 Add `backend/tests/route_guards_integration.rs` (D2), which walks every operation in `openapi.json` and checks all three things from D2. Verify it passes, that it fails when `require_auth()` is removed from the `profile` group on a scratch commit, and that it fails when a test-only unguarded route is added. Revert both
- [x] 1.6 Link the route-guard tests from the matching threat-model properties, and verify `just threat-model-check` passes
- [x] 1.7 Link the threat model from README.md and backend/ARCHITECTURE.md, including how to add a property and a check. Verify the links resolve
- [x] 1.8 Run `/code-review high` on the slice and fix the findings. Verify `just ci` is green
- [x] 1.9 Record in design.md Hypothesis what slice 1 found: the number of gaps, what kind they are, and any surprises. Reorder the remaining slices if the gaps call for it, then add slice 2 to this file in detail

## 2. Slice 2: close the gaps that need only ordinary tests

This slice delivers, from specs/security/spec.md:

- A Member can reach only their own account: both scenarios.
- The sign-in protocols resist an active network attacker: "A cross-site
  request is refused", for the CSRF-exempt routes, by example. The
  attacker-model proof is slice 5.

It also closes the threat-model gaps on P-04, P-09, P-17, P-21 and P-27. Each
task links its test from the property and removes that property's `Gap:`
line, or narrows it if part of the gap stays open. If a test shows the code
doesn't keep the property, the fix is part of the task, and the defect is
recorded in design.md Decisions.

- [x] 2.1 P-04: add an integration test that signs in, completes a password reset through the real flow, then sends the session token issued before the reset to a sensitive route (`GET /users/me/export`) and to `/auth/me`, and also tries the old refresh token. Verify the sensitive route answers 401 and the old refresh token is refused. Record what `/auth/me` does: it has no fresh-session check, so the old token works there until it expires. Either accept that in the catalogue with the reason, or raise it before changing behaviour
- [x] 2.2 P-17: add an integration test that signs in, signs out, then presents the old refresh cookie to `/auth/refresh`. Verify it's refused and issues no new session
- [x] 2.3 P-09: add an integration test with two verified Members, A and B. As A, call every `/users` route, putting B's id and email in the query and in extra body fields. Verify each call acts on A only (B's row unchanged, checked in the database) and that A's export holds no B data. Verify by also running it with B's data made distinctive (names, linked Google identity)
- [x] 2.4 P-21: add an integration test that sends each CSRF-exempt `POST` route a `text/plain` body and an `application/x-www-form-urlencoded` body (what a cross-site form can send without a preflight), and a CORS preflight from another origin. Verify the bodies are refused before any handler logic (no cookies set, no account created, no code sent), and that the preflight gets no `Access-Control-Allow-Origin` for the other origin. If a route accepts one of those bodies, fix the extractor, record the defect, and verify the fix
- [x] 2.5 P-27: add an integration test that runs sign-up, verification, sign-in, refresh and a password reset, keeping every raw password, code, magic-link token and other token it handles. Dump every row of every table while each code is live, and again at the end, and verify none of those values appears. Google linking is left out because it stores no secret, only the provider's user id and email; P-27 says so
- [x] 2.6 Update the five properties in `docs/security/threat-model.md` (checks added, gaps removed or narrowed, strength), and verify `just threat-model-check` passes
- [x] 2.7 Run `/code-review high` on the slice and fix the findings. Verify `just ci` is green
- [x] 2.8 Record what slice 2 showed in design.md Hypothesis, and add slice 3 (Kani, design D3–D7) to this file in detail

## 3. Slice 3: the code-entry lockout is proven for every input

This slice delivers, from specs/security/spec.md:

- Code-entry lockout holds for every input: all five scenarios.
- Auth guarantees are machine-checked on every change: "Breaking a guarantee
  fails the gate" and "Bounds and assumptions are visible", for the lockout
  proofs.

It closes the threat-model gaps on P-10 and P-13 (P-13 is the counter-wrap
defect). It also tests the riskiest unknown for the Kani slices: whether Kani
can build this crate and reason about its date-time arithmetic in reasonable
time.

- [x] 3.1 Install a pinned Kani (`"cargo:kani-verifier"` in `.mise.toml`, then `cargo kani setup`), add `cfg(kani)` to `check-cfg` in `backend/Cargo.toml`, and add one trivial `#[kani::proof]` in `attempt_lockout.rs`. Verify that `SQLX_OFFLINE=true cargo kani` builds the crate and passes, and record the build time in design.md Hypothesis
- [x] 3.2 (Not done: its condition wasn't met.) Only if 3.1 fails or the build takes more than about 5 minutes: extract `src/domain` into a workspace crate `backend/crates/domain` (design D4 fallback) and update ARCHITECTURE.md. Verify `just test` and `just lint` pass and `cargo kani -p <domain crate>` runs the trivial proof
  - Not needed and moved out of this change: Kani built the crate in place (3.1). Extracting `src/domain` into its own crate is a separate change, by the user's decision.
- [x] 3.3 Make `AttemptLockout::{seconds_until_unlock, can_resend, seconds_until_resend, reissue}` and the `reissue` delegates on `PasswordResetCode` and `EmailVerificationCode` take `now: DateTime<Utc>` (design D3), and update their application-layer callers to pass `Utc::now()`. Verify `just test` and `just test-integration` pass unchanged
- [x] 3.4 Add lock-window harnesses for every `lockouts: i32`: `lock_duration()` doesn't panic or overflow, falls within [15 min, 24 h], and is monotonic (for `a <= b`, `d(a) <= d(b)`). Doc comments state the bounds. Verify each passes with `cargo kani --harness`, and that each fails when `MAX_LOCK_DURATION_MINUTES` or the clamp is broken on a scratch edit
- [x] 3.5 Add reissue harnesses over a symbolic `attempts`, `lockouts`, and send and now timestamps bounded to 2000–2200 (design D5): `Locked` whenever locked and inside the window, with `seconds_remaining > 0`; `Cooldown` when not locked and less than 5 minutes since the send; `Allowed` once both waits have passed; every reported wait `>= 0`. If chrono arithmetic is intractable, use the D5 `i64` kernel fallback and record it. Verify each harness passes, and each fails on a scratch edit that breaks its rule
- [x] 3.6 Add a harness that calls `increment_attempts` on a code at a symbolic `attempts >= MAX_ATTEMPTS` and asserts it's still locked. Verify it fails today (wrap at `i32::MAX`), then change both entities to `saturating_add` (design D6) and verify it passes, adding a unit test at `i32::MAX`. (After the slice: `increment_attempts` was deleted, since it had no production caller; see design D6.)
- [x] 3.7 Add `just verify` (design D4) and add it to `ci`. Add a `verify` job to `.github/workflows/ci.yml` under the `changes.backend` filter, with Kani pinned to the same version. Verify `just verify` fails with a counterexample on a scratch break, then revert
- [x] 3.8 Link the harnesses from P-10 and P-13, set their strength to `proven`, and remove the gaps. Document `just verify`, the Kani install, and what the proofs cover (with bounds) in README.md and backend/ARCHITECTURE.md. Verify `just threat-model-check` passes
- [x] 3.9 Run `/code-review high` on the slice and fix the findings. Verify `just ci` is green
- [x] 3.10 Record what slice 3 showed (build time, solver time, whether the D4 or D5 fallbacks were needed) in design.md Hypothesis, and add slice 4 to this file in detail

## 4. Slice 4: concurrency is modelled, and the resend race is fixed

This slice delivers, from specs/security/spec.md:

- Concurrent requests cannot get around auth limits: all three scenarios.
- Auth guarantees are machine-checked on every change: "A model that drifts
  from the code fails the gate", for the two models.

It closes the threat-model gaps on P-11 (the resend race slice 3's review
found), P-12 and P-14. It tests the riskiest unknown for the model slices:
whether ITF traces from a Quint model can be replayed against the real
repositories with real concurrent connections, so the model can't drift from
the code.

- [x] 4.1 Fix the resend race first (P-11): the user chose to fix it before modelling it. First add an integration test that runs a 5th wrong guess and a resend concurrently many times, and verify it fails on the current unguarded upsert, with the lock overwritten. Then make the issuing upsert conditional, so it refuses to replace a row that's now at the cap and inside its window. Do it in SQL in both repositories (an `ON CONFLICT … DO UPDATE … WHERE` guard), and have the use cases report the existing `Locked` outcome when it refuses. Run `just db-prepare` and commit `backend/.sqlx/`. No migration is needed. Verify the test now passes every time
  - Staged, not committed: the user commits. The race test also covers the re-register path (`send_verification_code`), and the model later found a second defect in the same flow (the attempt claim named only the user), fixed in 4.3; see design.md Hypothesis.
- [x] 4.2 Pin Quint in `.mise.toml` (`"npm:@informalsystems/quint"`, exact version) and add a trivial `models/attempt_claim.qnt` with one invariant. Verify `quint verify` passes (or `quint run` with an invariant, if the Apalache backend needs Java the machine lacks), then fails on a deliberate break. Record the run time and backend in design.md Hypothesis
- [x] 4.3 Model the code-entry protocol in `models/attempt_claim.qnt` (design D8). Each request is the sequence of atomic database steps it really runs: the attempt claim (`SET attempts = attempts + 1 … WHERE attempts < 5`), the comparison, delete on success, expiry cleanup, and the issuing paths' read → decide → conditional upsert from 4.1. Invariants: at most 5 comparisons per code; a guess racing a successful verification or cleanup gets the no-code result; a code locked by a guess is never replaced inside its window. Verify the last invariant fails with the old unguarded upsert modelled (a counterexample trace showing the P-11 race) and holds with 4.1's guard
- [x] 4.4 Model refresh rotation in `models/refresh_rotation.qnt`: rotate, reuse detection (revoke all), logout and expiry, with concurrent presentations of one token. Invariants: at most one live successor per token, and any reuse after a successful rotation revokes every session. Verify each invariant fails on a deliberate break
- [x] 4.5 Conformance: export ITF traces (`quint run --out-itf`) for both models, and add a `#[sqlx::test]` suite that replays each trace's steps against the real repositories with concurrent connections and asserts the database reaches the state the model predicts (the `itf` crate reads the traces). Verify a deliberate change to a repository's SQL fails the suite, naming the step
- [x] 4.6 Add `just models` (`quint` checks plus the conformance suite), add it to `ci`, and add a CI job under the `changes.backend` filter with Quint pinned to the same version. Verify it fails on a scratch break, then revert
- [x] 4.7 Link the models and conformance tests from P-11, P-12 and P-14, set their strength to `modelled`, and remove or narrow their gaps. Document `just models` in README.md and backend/ARCHITECTURE.md. Verify `just threat-model-check` passes
- [x] 4.8 Run `/code-review high` on the slice and fix the findings. Verify `just ci` is green
  - Review run, valid findings fixed (see design.md Hypothesis). Not yet verified: `just ci`, and the post-review `just models` (its Apalache checks passed; the run was stopped for low memory during the random search, before the replay).
- [x] 4.9 Record what slice 4 showed in design.md Hypothesis, and add slice 5 (Tamarin, design D9) to this file in detail

## 5. Slice 5: the sign-in protocols are proven against an active attacker

This slice delivers, from specs/security/spec.md:

- The sign-in protocols resist an active network attacker: all four
  scenarios.
- Auth guarantees are machine-checked on every change: "Breaking a guarantee
  fails the gate" and "Bounds and assumptions are visible", for the protocol
  model.

It closes the protocol-level gaps on P-20, P-21 and P-22, which slice 1 found
are tested message by message only, and adds the attacker-model proof to
P-15 and P-16. It tests the riskiest unknown for this slice: whether Tamarin
terminates on refresh rotation with revocation, which is stateful (design D9;
ProVerif is the fallback). Slice 4's lesson applies: every lemma fails on a
deliberate break before it's trusted, and the old behaviour, where there is
one, stays in the model as a variant that must still be attacked.

- [x] 5.1 Pin a Tamarin Docker image by digest (design D9: mise has no Tamarin package), add `just protocols` (runs `tamarin-prover --prove models/auth_protocols.spthy` in that image), and a trivial `models/auth_protocols.spthy` with one lemma over one rule. Verify `just protocols` proves it, then fails with an attack trace on a deliberate break. Record the image, its digest, the Tamarin version and the run time in design.md Hypothesis. If no maintained image exists, stop and record the choice before building one
  - No maintained image exists. The user chose mise's `github:` backend over building an image (design D9), so Tamarin and Maude are pinned in `.mise.toml` and `just protocols` runs `scripts/protocols.sh` natively.
- [x] 5.2 Model password sign-in and sessions (design D9): Dolev-Yao attacker, plus an attacker with their own account; TLS as a secure channel; cookies as browser state bound to an origin; the session token and refresh token issued over that channel. Lemmas: a session or refresh token is obtainable only by the Member who signed in, and a replayed sign-in message gains nothing (scenario "A replayed sign-in message gains nothing", for sign-in). Verify each lemma is proven, and is attacked when the channel is made insecure on a scratch edit. Record the proof time
- [x] 5.3 Model refresh with rotation and reuse detection as Tamarin state (a token is live, rotated or revoked; reuse revokes the family), with the device fingerprint bound to the token. Lemmas: a stolen refresh token used after its owner refreshed ends every session (scenario "Stolen refresh token reuse ends every session", P-15), a replayed refresh gains nothing, and a token presented without its fingerprint yields nothing (P-16). State the bound slice 4 recorded: a token deleted by expiry cleanup is outside the lemma. Verify each lemma fails when its check (revoke-all, fingerprint) is removed on a scratch edit. If Tamarin doesn't terminate, try the lemma with a helper (source) lemma first, then the ProVerif fallback, and record which was needed
  - Revoke-all is a restriction (no token minted before it rotates after it), not epoch state: epochs needed induction that didn't terminate. Helper lemmas were enough; ProVerif wasn't needed. See design.md Hypothesis.
- [x] 5.4 Model Google sign-in: the state cookie set when sign-in starts, the state echoed by Google's redirect, the device fingerprint, and the callback's check that the state matches the cookie. Lemma: a sign-in completes only in the browser that started it, so an attacker can't sign a victim into the attacker's account (scenario "Login CSRF through Google sign-in fails", P-22). Verify the lemma is attacked when the state check is removed on a scratch edit, with the login-CSRF trace
- [x] 5.5 Model the CSRF double-submit: the readable CSRF cookie bound to the session's user, the header the frontend copies it into, and a cross-site page that can make the browser send requests but can't read the cookie. Lemmas: a state-changing request is accepted only with a header equal to the cookie and bound to the session's user (P-20), and the CSRF-exempt pre-auth routes can't be driven cross-site (P-21; within the modelled browser, which follows `SameSite=Lax` and sends no non-simple cross-site body without a preflight). Verify each fails on a scratch edit that drops the check
  - P-20 is two lemmas: no cross-site change without theft or the password, and the user binding, which still holds without the `__Host-` prefix (`NO_HOST_PREFIX`) and is attacked without it.
- [x] 5.6 Conformance (design D9): every rule in the model that encodes a check the code must make (state matches the cookie, CSRF header equals the cookie, refresh token not revoked, fingerprint matches) names, in a comment on the rule, the integration test that shows the code makes it. Extend `just threat-model-check` (or a sibling check it runs) so each such name must resolve to an existing test function. Verify by renaming one linked test on a scratch edit: the check fails, naming the rule
  - Two linked checks had only unit tests; `reusing_a_rotated_refresh_token_ends_every_session` and `a_refresh_token_is_refused_from_another_device` were added to `auth_properties_integration.rs`, each scratch-checked.
- [x] 5.7 Add `just protocols` to `ci`, and a CI job running the same pinned image under a paths filter for `models/**`, `routes.rs`, `cookies.rs` and the auth handlers. Verify it fails on a scratch break, then revert. If the proofs take more than about 10 minutes, run them only when those inputs change (design Risks) and record it
  - Done locally: `protocols` is in `just ci`, the `protocols` job is in ci.yml, and the image build waits on it. The job's paths filter is narrower than this task says (`models/*.spthy`, `scripts/protocols.sh`, shared files): code review pointed out the proofs can't change with the auth code, and the tests the rules name are resolved by the always-run threat-model job. Verified on GitHub: the `Protocol model (Tamarin)` job passed in CI run 43943987060 on main (2026-11-16).
- [x] 5.8 Link the lemmas and their named tests from P-15, P-16, P-20, P-21 and P-22, set strength to `modelled` where a lemma and its rule-linked tests exist, and remove or narrow the gaps, keeping the attacker assumptions (TLS as a secure channel, the modelled browser) next to each. Document `just protocols`, the image pin, and what is and isn't proven in README.md and backend/ARCHITECTURE.md. Verify `just threat-model-check` passes
  - The sign-in lemmas got a new property, P-33. P-22's cookie-tossing gap was closed by 5.11.
- [x] 5.9 Run `/code-review high` on the slice and fix the findings. Verify `just ci` is green
  - Ten findings, all fixed (design.md Hypothesis). `just ci` passed; it overlapped the review fixes, which touched only scripts, the model and docs, so `just protocols`, `just threat-model-check` and `just fmt-check` were rerun after them and pass.
- [x] 5.10 Record what slice 5 showed in design.md Hypothesis (Tamarin or ProVerif, proof times, helper lemmas needed, attacks found)
- [x] 5.11 Fix the model's finding (the user chose the `__Host-` prefix over accepting the gap): give every auth cookie the `__Host-` prefix with `Secure`, `Path=/` and no `Domain`, in dev too; rename it in the frontend and the tests. Add `auth_cookies_are_host_only` (every cookie set or cleared across sign-in, refresh, sign-out and Google sign-in) and link it from the rule where the browser stores the app's cookies. Keep the old code in the model as `NO_HOST_PREFIX`, with session-cookie tossing added, and require login CSRF to stay attacked there. Verify the new test fails when one cookie loses its prefix, and that a real browser keeps and sends the cookies over `http://localhost` (`tests/integration/signup.spec.ts`)

## 6. Slice 6: the rest of the domain policy is proven for every input

This slice delivers, from specs/security/spec.md, "for every input":

- Role management never escalates privilege: all three scenarios.
- One-time codes are accepted only while live: both scenarios.
- A revoked refresh token is never accepted: both scenarios.
- A deleted account stays deleted: all three scenarios.
- Email addresses normalise to one identity: both scenarios (short ASCII, D7).
- Auth guarantees are machine-checked on every change: "Breaking a guarantee
  fails the gate" and "Bounds and assumptions are visible", for these proofs.

Slices 3–5 proved only the lockout policy, leaving P-08, P-19, P-28 and P-30
marked "Kani proof planned (OT-010)", and the refresh-token status rule
(P-15) tested but not proven. The user chose to finish them here rather than
move them out. Slice 3's pattern applies: time is an argument (D3), the proven
rule is an integer kernel over microseconds that the chrono method calls (D5),
harnesses sit beside the code, and each fails on a scratch break before it's
trusted.

- [x] 6.1 Roles (P-08): move the use case's authority decision into the domain as a pure function over (actor role, acting on self, current role, new role) that `SetUserRoleUseCase` calls, keeping its error messages. Harnesses over every combination: a Member manages no one; an Admin only sets a Member to Member; only a Super Admin touches an Admin or Super Admin, as current or new role; a Super Admin never removes their own role. Add a harness over every string up to 12 bytes: `from_db_str` accepts exactly the three role names, and every role round-trips. The last-Super-Admin guard stays in SQL and keeps its tests. Verify `just test` passes and each harness fails on a scratch break
  - `UserRole::may_change_role`, called by `SetUserRoleUseCase`. The role-name proof is limited to ASCII (non-ASCII can't equal an ASCII name; symbolic UTF-8 took 109 s) and takes about 76 s, the slowest proof. Each harness was scratch-checked.
- [x] 6.2 One-time codes (P-19): make `is_expired`, `is_valid` and `is_magic_token_expired` on both code entities take `now` (D3), with callers passing `Utc::now()`, over an integer kernel. Harnesses over any expiry, attempt count and time in [2000, 2200): a code at or after its expiry is refused whatever its attempts; a code at the cap is refused before its expiry; a code is accepted exactly when it's before expiry and below the cap; a magic link is live exactly before its own expiry, whatever the code's expiry. Verify `just test` and `just test-integration` pass and each harness fails on a scratch break
  - Proven: the expiry rule both code entities call (`attempt_lockout::expired`, shared with refresh tokens) and the magic-link rule (`magic_link_expired`, a missing link counts as expired). Review: an earlier `accepts_entry` proof covered a combination no request path used, so it was dropped. Scratch-checked (`>` for `>=`; a missing link counted live).
- [x] 6.3 Refresh tokens (P-15): make `status` and `revoke` take `now` over an integer kernel. Harnesses: a revoked token reports `Revoked` at any time, before or after its expiry; a token is `Active` exactly when unrevoked and before expiry; revoking twice keeps the first revocation time. Verify as in 6.2
  - `status_at` and a generic `first_revocation` that `revoke` calls. Scratch-checked (expired outranking revoked; revoke overwriting).
- [x] 6.4 Deletion (P-28): make the `AccountStatus` transitions (schedule, cancel, anonymise, ready to purge) take `now`, with `User` delegating, over an integer kernel if chrono is intractable. Harnesses over every status and time: from `Deleted`, every action is refused and the status and deletion time don't change; scheduling while pending keeps the date; a pending deletion is ready to purge exactly when its time has passed; `from_columns` reads deleted when both columns are set, and every status round-trips through its columns. Verify as in 6.2
  - No integer kernel needed: `AccountStatus` is generic over its timestamp (default `DateTime<Utc>`), so the proofs run the same transitions over `i64`. `is_ready_for_deletion` has no production caller; the purge job's due check is SQL, so P-28 says that part is tested, not proven. Four breaks scratch-checked.
- [x] 6.5 Email (P-30, D7): harnesses over inputs of up to 8 ASCII bytes: normalising an accepted address again gives the same address, and two inputs that differ only in case or surrounding whitespace give the same result or are both refused. Verify each fails on a scratch break (for example, dropping `trim`)
  - Building an `Email` (and Unicode `to_lowercase`) didn't finish in 15 and 20 minutes. `Email::new` now takes a byte-wise ASCII path (`ascii_trim_range`) for ASCII input, with a proptest and a vertical-tab test showing it equals `trim().to_lowercase()`. The proofs cover the normalisation, since `Email::new` validates the normalised string: 23 s and 13 s. Scratch-checked (vertical tab dropped; first byte dropped).
- [x] 6.6 Link the harnesses from P-08, P-15, P-19, P-28 and P-30, set strength to `proven` where every case of the property is proven, and remove or narrow the gaps, stating each proof's bounds. Update "What's proven" for `just verify` in README.md and backend/ARCHITECTURE.md, with the new run time. Verify `just threat-model-check` passes
  - P-08, P-15, P-19 and P-28 are `proven, tested`, each stating the part left to SQL tests. P-30 keeps a narrowed gap: proven for ASCII up to 8 bytes only. `just verify`: 21 harnesses in about 2 minutes warm (was about 15 s).
- [x] 6.7 Run `/code-review high` on the slice and fix the findings. Verify `just ci` is green
  - Nine findings. Fixed: P-19 claimed a proof of a rule no path calls (now the two rules they do call, plus a magic-link proof the task asked for); a correct code could be accepted after expiring during its Argon2 check (re-checked after the check in both flows, two tests that fail without it; the user chose to fix it, and the SQL claim was the wrong place since it runs before the check); the expiry rule and time bounds duplicated in `refresh_token`; the Super-Admin-removal rule duplicated in the use case; task 3.2's mark. Kept: the 76 s role-name proof (the spec says every input; CI runs it in parallel) and the generic `AccountStatus` (it proves the real transitions; recorded in design.md). Noted: expiry now compares to the microsecond, Postgres's precision.
- [x] 6.8 Record what slice 6 showed in design.md Hypothesis (proof times, which kernels were needed, any defects found)

## 7. Feature done

- [x] 7.1 Verify `docs/security/threat-model.md` has no property without a check, except accepted gaps with a reason, and that every scenario in the spec delta is covered by a check `just ci` runs
  - No property says "planned (OT-010)" any more. Eight gaps remain, each with a reason or a home: P-03 (edge rate limiting, first deploy), P-12, P-15, P-17, P-21 (accepted), P-30 (ASCII up to 8 bytes, D7), P-31 and P-32 (OT-011). Every requirement maps to properties in the threat model's Spec coverage table, and `just ci` was green on 2026-11-17 (21 Kani harnesses, models, protocols, 806 integration tests, e2e).
- [x] 7.2 Check README.md and backend/ARCHITECTURE.md describe every verification recipe, the tools and their pins, and what is and isn't proven. Verify by following the README setup from a clean checkout
  - Found and fixed: `just setup` didn't run `cargo kani setup`, so a fresh checkout's `just ci` failed at `just verify`. README gained "What's proven about auth". Followed the README from a clean `just wt-new` checkout of 21b772d (2026-11-17): `mise install`, `just setup`, then `threat-model-check`, `verify` (185 s cold), `protocols` (86 s) and `models` (335 s) all passed.
- [x] 7.3 Record the external penetration test (scoped by the threat model) with the first-deploy notes, and verify it's listed there
  - Listed under "Security (OT-010), before launch" in `deploy/compose/README.md` (First deploy), next to P-03's edge rate limiting, which the threat model already expects there.
- [x] 7.4 Archive the change with `/opsx:archive` and verify `openspec/specs/security/spec.md` contains the new requirements
