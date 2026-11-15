# Tasks

Slices are planned in detail one at a time: the next is added once the
current one ships and design.md Hypothesis records what it showed. After
slice 2, the order (reordered by slice 1's findings) is:

3. domain proofs with Kani (design D3–D7)
4. the Quint concurrency model (D8)
5. the Tamarin protocol model (D9)
6. fuzzing (D10)

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

- [ ] 2.1 P-04: add an integration test that signs in, completes a password reset through the real flow, then sends the session token issued before the reset to a sensitive route (`GET /users/me/export`) and to `/auth/me`, and also tries the old refresh token. Verify the sensitive route answers 401 and the old refresh token is refused. Record what `/auth/me` does: it has no fresh-session check, so the old token works there until it expires. Either accept that in the catalogue with the reason, or raise it before changing behaviour
- [ ] 2.2 P-17: add an integration test that signs in, signs out, then presents the old refresh cookie to `/auth/refresh`. Verify it's refused and issues no new session
- [ ] 2.3 P-09: add an integration test with two verified Members, A and B. As A, call every `/users` route, putting B's id and email in the query and in extra body fields. Verify each call acts on A only (B's row unchanged, checked in the database) and that A's export holds no B data. Verify by also running it with B's data made distinctive (names, linked Google identity)
- [ ] 2.4 P-21: add an integration test that sends each CSRF-exempt `POST` route a `text/plain` body and an `application/x-www-form-urlencoded` body (what a cross-site form can send without a preflight), and a CORS preflight from another origin. Verify the bodies are refused before any handler logic (no cookies set, no account created, no code sent), and that the preflight gets no `Access-Control-Allow-Origin` for the other origin. If a route accepts one of those bodies, fix the extractor, record the defect, and verify the fix
- [ ] 2.5 P-27: add an integration test that runs sign-up, verification, sign-in, refresh, a password-reset request and a Google link (the fake provider), keeping every raw password, code, token and OAuth secret it handles. Then dump every row of every table as text and verify none of those values appears in it
- [ ] 2.6 Update the five properties in `docs/security/threat-model.md` (checks added, gaps removed or narrowed, strength), and verify `just threat-model-check` passes
- [ ] 2.7 Run `/code-review high` on the slice and fix the findings. Verify `just ci` is green
- [ ] 2.8 Record what slice 2 showed in design.md Hypothesis, and add slice 3 (Kani, design D3–D7) to this file in detail

## 3. Feature done

- [ ] 3.1 Verify `docs/security/threat-model.md` has no property without a check, except accepted gaps with a reason, and that every scenario in the spec delta is covered by a check `just ci` runs
- [ ] 3.2 Check README.md and backend/ARCHITECTURE.md describe every verification recipe, the tools and their pins, and what is and isn't proven. Verify by following the README setup from a clean checkout
- [ ] 3.3 Record the external penetration test (scoped by the threat model) with the first-deploy notes, and verify it's listed there
- [ ] 3.4 Archive the change with `/opsx:archive` and verify `openspec/specs/security/spec.md` contains the new requirements
