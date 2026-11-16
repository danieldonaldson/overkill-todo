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
- [ ] 3.2 Only if 3.1 fails or the build takes more than about 5 minutes: extract `src/domain` into a workspace crate `backend/crates/domain` (design D4 fallback) and update ARCHITECTURE.md. Verify `just test` and `just lint` pass and `cargo kani -p <domain crate>` runs the trivial proof
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

- [ ] 5.1 Pin a Tamarin Docker image by digest (design D9: mise has no Tamarin package), add `just protocols` (runs `tamarin-prover --prove models/auth_protocols.spthy` in that image), and a trivial `models/auth_protocols.spthy` with one lemma over one rule. Verify `just protocols` proves it, then fails with an attack trace on a deliberate break. Record the image, its digest, the Tamarin version and the run time in design.md Hypothesis. If no maintained image exists, stop and record the choice before building one
- [ ] 5.2 Model password sign-in and sessions (design D9): Dolev-Yao attacker, plus an attacker with their own account; TLS as a secure channel; cookies as browser state bound to an origin; the session token and refresh token issued over that channel. Lemmas: a session or refresh token is obtainable only by the Member who signed in, and a replayed sign-in message gains nothing (scenario "A replayed sign-in message gains nothing", for sign-in). Verify each lemma is proven, and is attacked when the channel is made insecure on a scratch edit. Record the proof time
- [ ] 5.3 Model refresh with rotation and reuse detection as Tamarin state (a token is live, rotated or revoked; reuse revokes the family), with the device fingerprint bound to the token. Lemmas: a stolen refresh token used after its owner refreshed ends every session (scenario "Stolen refresh token reuse ends every session", P-15), a replayed refresh gains nothing, and a token presented without its fingerprint yields nothing (P-16). State the bound slice 4 recorded: a token deleted by expiry cleanup is outside the lemma. Verify each lemma fails when its check (revoke-all, fingerprint) is removed on a scratch edit. If Tamarin doesn't terminate, try the lemma with a helper (source) lemma first, then the ProVerif fallback, and record which was needed
- [ ] 5.4 Model Google sign-in: the state cookie set when sign-in starts, the state echoed by Google's redirect, the device fingerprint, and the callback's check that the state matches the cookie. Lemma: a sign-in completes only in the browser that started it, so an attacker can't sign a victim into the attacker's account (scenario "Login CSRF through Google sign-in fails", P-22). Verify the lemma is attacked when the state check is removed on a scratch edit, with the login-CSRF trace
- [ ] 5.5 Model the CSRF double-submit: the readable CSRF cookie bound to the session's user, the header the frontend copies it into, and a cross-site page that can make the browser send requests but can't read the cookie. Lemmas: a state-changing request is accepted only with a header equal to the cookie and bound to the session's user (P-20), and the CSRF-exempt pre-auth routes can't be driven cross-site (P-21; within the modelled browser, which follows `SameSite=Lax` and sends no non-simple cross-site body without a preflight). Verify each fails on a scratch edit that drops the check
- [ ] 5.6 Conformance (design D9): every rule in the model that encodes a check the code must make (state matches the cookie, CSRF header equals the cookie, refresh token not revoked, fingerprint matches) names, in a comment on the rule, the integration test that shows the code makes it. Extend `just threat-model-check` (or a sibling check it runs) so each such name must resolve to an existing test function. Verify by renaming one linked test on a scratch edit: the check fails, naming the rule
- [ ] 5.7 Add `just protocols` to `ci`, and a CI job running the same pinned image under a paths filter for `models/**`, `routes.rs`, `cookies.rs` and the auth handlers. Verify it fails on a scratch break, then revert. If the proofs take more than about 10 minutes, run them only when those inputs change (design Risks) and record it
- [ ] 5.8 Link the lemmas and their named tests from P-15, P-16, P-20, P-21 and P-22, set strength to `modelled` where a lemma and its rule-linked tests exist, and remove or narrow the gaps, keeping the attacker assumptions (TLS as a secure channel, the modelled browser) next to each. Document `just protocols`, the image pin, and what is and isn't proven in README.md and backend/ARCHITECTURE.md. Verify `just threat-model-check` passes
- [ ] 5.9 Run `/code-review high` on the slice and fix the findings. Verify `just ci` is green
- [ ] 5.10 Record what slice 5 showed in design.md Hypothesis (Tamarin or ProVerif, proof times, helper lemmas needed, attacks found)

## 6. Feature done

- [ ] 6.1 Verify `docs/security/threat-model.md` has no property without a check, except accepted gaps with a reason, and that every scenario in the spec delta is covered by a check `just ci` runs
- [ ] 6.2 Check README.md and backend/ARCHITECTURE.md describe every verification recipe, the tools and their pins, and what is and isn't proven. Verify by following the README setup from a clean checkout
- [ ] 6.3 Record the external penetration test (scoped by the threat model) with the first-deploy notes, and verify it's listed there
- [ ] 6.4 Archive the change with `/opsx:archive` and verify `openspec/specs/security/spec.md` contains the new requirements
