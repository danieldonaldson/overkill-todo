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

- [ ] 3.1 Install a pinned Kani (`"cargo:kani-verifier"` in `.mise.toml`, then `cargo kani setup`), add `cfg(kani)` to `check-cfg` in `backend/Cargo.toml`, and add one trivial `#[kani::proof]` in `attempt_lockout.rs`. Verify that `SQLX_OFFLINE=true cargo kani` builds the crate and passes, and record the build time in design.md Hypothesis
- [ ] 3.2 Only if 3.1 fails or the build takes more than about 5 minutes: extract `src/domain` into a workspace crate `backend/crates/domain` (design D4 fallback) and update ARCHITECTURE.md. Verify `just test` and `just lint` pass and `cargo kani -p <domain crate>` runs the trivial proof
- [ ] 3.3 Make `AttemptLockout::{seconds_until_unlock, can_resend, seconds_until_resend, reissue}` and the `reissue` delegates on `PasswordResetCode` and `EmailVerificationCode` take `now: DateTime<Utc>` (design D3), and update their application-layer callers to pass `Utc::now()`. Verify `just test` and `just test-integration` pass unchanged
- [ ] 3.4 Add lock-window harnesses for every `lockouts: i32`: `lock_duration()` doesn't panic or overflow, falls within [15 min, 24 h], and is monotonic (for `a <= b`, `d(a) <= d(b)`). Doc comments state the bounds. Verify each passes with `cargo kani --harness`, and that each fails when `MAX_LOCK_DURATION_MINUTES` or the clamp is broken on a scratch edit
- [ ] 3.5 Add reissue harnesses over a symbolic `attempts`, `lockouts`, and send and now timestamps bounded to 2000–2200 (design D5): `Locked` whenever locked and inside the window, with `seconds_remaining > 0`; `Cooldown` when not locked and less than 5 minutes since the send; `Allowed` once both waits have passed; every reported wait `>= 0`. If chrono arithmetic is intractable, use the D5 `i64` kernel fallback and record it. Verify each harness passes, and each fails on a scratch edit that breaks its rule
- [ ] 3.6 Add a harness that calls `increment_attempts` on a code at a symbolic `attempts >= MAX_ATTEMPTS` and asserts it's still locked. Verify it fails today (wrap at `i32::MAX`), then change both entities to `saturating_add` (design D6) and verify it passes, adding a unit test at `i32::MAX`
- [ ] 3.7 Add `just verify` (design D4) and add it to `ci`. Add a `verify` job to `.github/workflows/ci.yml` under the `changes.backend` filter, with Kani pinned to the same version. Verify `just verify` fails with a counterexample on a scratch break, then revert
- [ ] 3.8 Link the harnesses from P-10 and P-13, set their strength to `proven`, and remove the gaps. Document `just verify`, the Kani install, and what the proofs cover (with bounds) in README.md and backend/ARCHITECTURE.md. Verify `just threat-model-check` passes
- [ ] 3.9 Run `/code-review high` on the slice and fix the findings. Verify `just ci` is green
- [ ] 3.10 Record what slice 3 showed (build time, solver time, whether the D4 or D5 fallbacks were needed) in design.md Hypothesis, and add slice 4 to this file in detail

## 4. Feature done

- [ ] 4.1 Verify `docs/security/threat-model.md` has no property without a check, except accepted gaps with a reason, and that every scenario in the spec delta is covered by a check `just ci` runs
- [ ] 4.2 Check README.md and backend/ARCHITECTURE.md describe every verification recipe, the tools and their pins, and what is and isn't proven. Verify by following the README setup from a clean checkout
- [ ] 4.3 Record the external penetration test (scoped by the threat model) with the first-deploy notes, and verify it's listed there
- [ ] 4.4 Archive the change with `/opsx:archive` and verify `openspec/specs/security/spec.md` contains the new requirements
