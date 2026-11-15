# OT-010: Provable auth security

## Why

As the maintainer, I want every security property the auth system relies on to
be written down and checked by the strongest method that fits it. Then "auth is
secure" means a specific list of guarantees, each with evidence, rather than a
feeling.

"Secure" has no single meaning, so nothing can prove it outright. What can be
proven is that the system keeps named properties against a named attacker. For
auth those properties are about who you are, what you may touch, sessions,
brute force, cross-site requests and secrecy. Today none of them are written
down in one place, and none are checked over every input. They are scattered
through unit and integration tests that sample cases. A first look shows what
sampling misses. The failed-attempt counter is a plain 32-bit increment, and
release builds don't check for overflow, so a counter pushed past its maximum
would wrap to a negative number and read as "not locked". It's unreachable
today only because the database caps attempts at 5. Nothing in the policy
itself rules it out.

## What Changes

- **A threat model and property catalogue.** It names the attackers this
  system defends against and numbers each security property. Each property
  links to the check that covers it. The gate fails if any property has no
  check, or names a check that doesn't exist.
- **Authorisation by construction.** Every route is either on an explicit
  public list or guarded, with role checks enforced at the route level. A route
  added without a guard fails the gate.
- **Domain policy proven for every input.** This covers lockout, cooldown,
  one-time codes, refresh-token revocation, role management, account deletion
  and email normalisation. Defects the proofs find get fixed, starting with the
  counter wrap.
- **A concurrency model** of the attempt claim and refresh-token rotation,
  model-checked over every interleaving of concurrent requests and tied to the
  code by conformance tests.
- **A protocol model** of sign-in, session refresh, Google sign-in and the CSRF
  defence. It is proven against an attacker who controls the network and can
  hold their own account.
- Time-dependent policy takes "now" as an input instead of reading the clock.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `security`: adds the threat-model and coverage requirement, plus
  requirements for route guarding, account isolation, the code-entry lockout,
  one-time codes, refresh tokens (including under concurrency), roles, account
  deletion, email identity and protocol soundness. All of them
  are machine-checked on every change.

## Impact

- Backend domain, application callers (time passed in), route wiring and tests.
- New tooling in the gate and CI: a bounded model checker, a TLA+-family model
  checker and a protocol prover, all pinned. None of them ship in the
  image.
- A new threat-model document, linked from README and ARCHITECTURE.md.
- No API, schema, frontend or deploy change is expected. If the proofs find
  defects, the fixes may change behaviour, and the specs below define what that
  behaviour must be.

## Out of Scope

- **Proving the cryptographic primitives.** These are trusted, and some
  already carry their vendor's machine-checked proofs. We check that they are
  used correctly, not that they are correct.
- **End-to-end code-level verification** (seL4-style). The models are tied to
  the code by conformance tests, not by refinement proofs.
- **The frontend, the edge proxy and infrastructure hardening.** Their checks
  are already in the `security` spec or belong to the first deploy.
- **Fuzzing and malformed-input handling.** OT-011 (API fuzzing) owns these.
  Threat-model properties P-31 and P-32 link to its checks once it lands.
- **An external penetration test.** It's still worth doing before launch,
  against the threat model this change produces.

## Dependencies

None to start. P-31 and P-32 (malformed input) are covered by OT-011's
checks once it lands. All the tools are development and CI only.

## Appetite

8 weeks.

## Shape of the work

Each part can be released on its own, in this order:

1. **Threat model, coverage map, route-guard gate and route-level admin
   roles.** This shows where the gaps are, so everything after it is sized
   against real gaps. Done.
2. **Close the gaps that need only ordinary tests**: session cutoff on
   password reset, account isolation, refresh after logout, cross-site
   requests on CSRF-exempt routes, and no plaintext at rest.
3. **Domain policy proofs**: lockout first, including the counter-wrap fix,
   then codes and tokens, then roles, deletion and email.
4. **Concurrency model** of the attempt claim and refresh rotation, with
   conformance tests.
5. **Protocol model** of sign-in, refresh, Google sign-in and CSRF.

Slice 1 reordered this (design.md, Hypothesis): the cheapest real gaps turned
out to be missing ordinary tests, not missing proofs.

## Ready

- **Riskiest unknown:** Which properties are uncovered today. Until the
  catalogue exists, we can't tell whether the remaining time should go to
  proofs, models or plain missing tests.
- **First slice:** Write the threat model and numbered property catalogue,
  link every property to its existing checks or mark it as a gap, and add the
  gates for the catalogue and for route guards.
