# Auth threat model

What "auth is secure" means for overkill-todo: the attackers we defend
against, what they're after, and a numbered list of properties the system must
keep. Each property names the checks that cover it, or says plainly that it
isn't covered yet. `just threat-model-check` (part of `just ci`) fails if a
property has neither, or if a check it names no longer exists.

This document covers authentication, authorisation, sessions and the data
they protect. Transport and secrets-at-rest cryptography are specified in
`openspec/specs/security/spec.md` (OT-009) and aren't repeated here.

## Attackers

| ID  | Attacker                | Can                                                                                                                                 | Can't                                                                                                                |
| --- | ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| A1  | Visitor on the internet | Send any HTTP request to the public edge, at volume; create their own accounts; run a website that a Member visits                  | Read TLS traffic; read server memory, logs or the database                                                           |
| A2  | Malicious Member        | Everything A1 can, while holding a valid session and verified account of their own                                                  | Act as another account without that account's credentials                                                            |
| A3  | Token thief             | Hold a copy of another Member's session cookie, refresh cookie or a one-time code or link (malware, a shared machine, a leaked log) | Read the victim's device fingerprint cookie, unless also stolen                                                      |
| A4  | Network attacker        | Read, drop, replay and change traffic that isn't protected by TLS; make a victim's browser visit any URL                            | Break TLS, or the HMAC-SHA-512 signatures                                                                            |
| A5  | Database reader         | Read a full copy of the database (a leaked backup or dump)                                                                          | Read the application's secrets (signing keys, the secrets key, the CSRF secret), which are held outside the database |

Out of scope: an attacker who controls the host or the deploy pipeline, the
Google account provider itself, or a Member's own unlocked device.

## Assets

- **Account control**: being able to act as a Member, Admin or Super Admin.
- **Account data**: profile, email and the exported data.
- **Credentials**: password hashes, one-time codes and links, refresh tokens,
  OAuth links and secrets, and the application's own keys.
- **Availability of an account**: a Member not being locked out or signed out
  by someone else.

## Trust boundaries

1. **Browser ↔ edge**: TLS; cookies `__Host-` prefixed, `Secure; Path=/;
SameSite=Lax`, and `HttpOnly` except the CSRF cookie; CORS allows only the
   frontend origin.
2. **Edge ↔ backend**: same-host proxy. The backend trusts the edge's
   forwarded headers only for tracing, never for auth.
3. **Backend ↔ database**: every auth decision is made in the backend.
   Concurrency limits are enforced by single SQL statements.
4. **Backend ↔ Google**: OAuth code exchange over TLS, and the ID token is
   verified against Google's keys.

## Check strength

Each property records how strongly it is checked:

- **tested**: example-based unit or integration tests.
- **property-tested**: proptest over random inputs (sampled, not exhaustive).
- **proven**: a proof over every input within stated bounds (Kani).
- **modelled**: a model checked over every interleaving or attacker action
  (Quint or Tamarin), tied to the code by conformance tests.

## Protocol model

`models/auth_protocols.spthy` (Tamarin, `just protocols`) proves the
properties marked "Modelled (Tamarin …)" below against this attacker and these
assumptions:

- **Attacker:** reads, replays and forges anything outside TLS (A4), holds their
  own account and Google identity, runs their own client, and makes a Member's
  browser open any URL or submit any cross-site form (A1). Where a lemma allows
  it, they also steal a browser's cookies, its device fingerprint or a password
  (A3).
- **Assumptions:** TLS is a secure channel. The MACs are perfect under keys
  outside the database: HMAC-SHA-512 for the session JWT and OAuth state,
  HMAC-SHA-256 for the CSRF token. Google is honest. Cookies are browser state
  bound to the app's host, which the `__Host-` prefix on every auth cookie
  makes the browser enforce (P-24). `-D=NO_HOST_PREFIX` is the code before the
  prefix, where the attacker can write the app's cookies; P-20 and P-22 say
  what holds then.
- **Bounds:** none on sessions, browsers, accounts or messages. Time, logout
  and concurrency are outside the model; Kani and Quint cover those.
- **Conformance:** each model rule that encodes a check the code makes names
  the integration test that shows it, and `just threat-model-check` resolves
  those names. `just protocols` also requires every lemma to be attacked when
  its check is removed.

## Properties

### Authentication

### P-01: A session token is accepted only if it is HMAC-SHA-512 signed by a trusted key, unexpired, and has the right issuer, audience and type

- Attackers: A1, A3, A4
- Strength: tested, property-tested
- Checks:
  - `backend/src/infrastructure/crypto/keyring.rs::refuses_alg_none`
  - `backend/src/infrastructure/crypto/keyring.rs::refuses_hs256_under_the_right_key`
  - `backend/src/infrastructure/crypto/keyring.rs::refuses_a_missing_or_unknown_kid_without_trying_other_keys`
  - `backend/src/infrastructure/crypto/keyring.rs::refuses_any_change_to_header_claims_or_signature`
  - `backend/src/infrastructure/crypto/keyring.rs::prop_random_input_never_verifies`
  - `backend/src/infrastructure/api/jwt.rs::test_rs256_token_rejected`
  - `backend/src/infrastructure/api/jwt.rs::test_wrong_issuer_rejected`
  - `backend/src/infrastructure/api/jwt.rs::test_wrong_audience_rejected`
  - `backend/src/infrastructure/api/jwt.rs::test_non_session_typ_rejected`
  - `backend/tests/security_integration.rs::expired_jwt_is_rejected`
  - `backend/tests/security_integration.rs::tampered_jwt_signature_is_rejected`

### P-02: Sign-in, sign-up and recovery answer the same way whether or not an account exists

- Attackers: A1
- Strength: tested
- Checks:
  - `backend/src/application/use_cases/auth/login_user.rs::unknown_email_runs_dummy_verify_and_is_invalid_credentials`
  - `backend/src/application/use_cases/auth/login_user.rs::passwordless_account_runs_dummy_verify_and_is_invalid_credentials`
  - `backend/tests/handler_branches_integration.rs::login_signup_incomplete_returns_generic_401`
  - `backend/tests/verification_integration.rs::register_existing_account_answers_like_a_new_signup`
  - `backend/tests/verification_integration.rs::forgot_password_unknown_email_returns_success`
  - `backend/tests/verification_integration.rs::resend_answers_identically_whatever_the_account_state`

### P-03: Password guessing is slowed by bot protection and hashing cost

- Attackers: A1
- Strength: tested
- Checks:
  - `backend/tests/turnstile_integration.rs::missing_token_is_rejected_on_every_protected_endpoint`
  - `backend/tests/turnstile_integration.rs::failed_verification_is_rejected_on_every_protected_endpoint`
  - `backend/tests/turnstile_integration.rs::upstream_failure_fails_closed_without_leaking_details`
- Gap: There is no per-account or per-address limit on password attempts in the application. Bot protection and argon2 cost are the only brakes, and a solver farm gets past bot protection. Rate limiting is expected at the edge (Traefik) at first deploy. Until then this is an accepted gap, not a covered property.

### P-04: A session issued before a password reset, account purge or role change is refused on `/auth/me`, Google unlink, account-sensitive and admin routes

- Attackers: A3
- Strength: tested
- Checks:
  - `backend/src/infrastructure/api/middleware/auth.rs::changed_role_invalidates_session`
  - `backend/src/infrastructure/api/middleware/auth.rs::token_from_cutoff_second_remains_revoked_after_role_cycle`
  - `backend/tests/admin_integration.rs::demoted_admin_token_is_rejected`
  - `backend/src/application/use_cases/auth/reset_password.rs::test_reset_password_success`
  - `backend/src/application/use_cases/user/change_password.rs::test_change_password_without_refresh_token_revokes_all`
  - `backend/tests/verification_integration.rs::password_reset_flow_succeeds`
  - `backend/tests/auth_properties_integration.rs::a_password_reset_ends_earlier_sessions`

Ordinary routes would trust a token until it expires, but there are none
today: `/auth/me`, Google unlink, the profile routes and the admin routes all
run the fresh-session check. A route added without it would serve a superseded
session for up to `JWT_EXPIRATION_MINUTES`.

### P-05: An account with an unverified email can't use profile or admin routes

- Attackers: A1
- Strength: tested
- Checks:
  - `backend/src/infrastructure/api/middleware/verification.rs::test_unverified_user_gets_403`
  - `backend/tests/security_integration.rs::unverified_session_blocked_by_verification_gate`
  - `backend/tests/verification_integration.rs::verification_gate_blocks_then_allows`

### Authorisation

### P-06: Every route is either on the public list by name or requires a valid session

- Attackers: A1
- Strength: tested
- Checks:
  - `backend/tests/route_guards_integration.rs::every_non_public_route_refuses_a_visitor`
  - `backend/tests/route_guards_integration.rs::every_public_route_still_exists`
  - `backend/tests/profile_integration.rs::export_data_without_token_returns_401`
  - `backend/tests/auth_integration.rs::me_without_token_returns_401`

The route-guard test walks every operation in the API description, so a new
route is checked automatically. The public list in that test is the one place
a route is declared public.

### P-07: Admin routes enforce the admin role before the handler runs

- Attackers: A2
- Strength: tested
- Checks:
  - `backend/tests/route_guards_integration.rs::every_admin_route_refuses_a_member`
  - `backend/tests/route_guards_integration.rs::admin_guards_refuse_members_without_any_handler_check`
  - `backend/tests/route_guards_integration.rs::a_just_promoted_member_is_sent_to_refresh_not_refused`
  - `backend/src/infrastructure/api/extractors/admin.rs::admin_user_with_plain_user_role_is_access_denied`
  - `backend/src/infrastructure/api/extractors/admin.rs::super_admin_user_with_admin_role_is_access_denied`
  - `backend/tests/admin_integration.rs::list_users_forbidden_for_non_admin`

The role is enforced twice: by `require_admin()` or `require_super_admin()`
on the route group (`admin_guards` / `super_admin_guards` in `routes.rs`), and
by the `AdminUser` or `SuperAdminUser` extractor on each handler. The
route-level check runs after the fresh-session check, so a Member who was just
promoted gets a 401 and refreshes, rather than a 403 from the stale role.

### P-08: Role management never escalates privilege

An Admin manages only Members; only a Super Admin manages Admins and Super
Admins; a Member manages no one. Both the target's current role and the new
role are checked. A Super Admin can't demote themselves or the last Super
Admin. An unknown stored role is rejected rather than defaulted.

- Attackers: A2
- Strength: tested, property-tested
- Checks:
  - `backend/src/domain/value_objects/user_role.rs::prop_user_manages_none`
  - `backend/src/domain/value_objects/user_role.rs::prop_super_admin_manages_all`
  - `backend/src/domain/value_objects/user_role.rs::prop_invalid_db_str_returns_none`
  - `backend/src/domain/value_objects/user_role.rs::prop_user_role_db_roundtrip`
  - `backend/src/application/use_cases/admin/manage_users.rs::set_user_role_forbids_admin_managing_admin`
  - `backend/src/application/use_cases/admin/manage_users.rs::set_user_role_forbids_self_demotion`
  - `backend/src/application/use_cases/admin/manage_users.rs::set_user_role_forbids_demoting_last_super_admin`
  - `backend/tests/admin_integration.rs::role_change_forbidden_for_admin`
- Gap: Sampled, not proven for every role pair. Kani proof planned (OT-010).

### P-09: A Member can reach only their own account

- Attackers: A2
- Strength: tested
- Checks:
  - `backend/tests/auth_properties_integration.rs::a_member_reaches_only_their_own_account`
  - `backend/tests/profile_integration.rs::export_data_returns_user_profile`

Every account route acts on the account named by the session. The isolation
test calls each `/users` route, and Google unlink, as one Member, with another
Member's id and email in the query and in extra body fields. The other Member
is password-less and has a linked Google identity, so a route that wrongly
acted on them would visibly succeed. The test checks that the other account's
stored data (user row, linked identity, sessions) is unchanged and never
appears in a response.

### Brute force on one-time codes

### P-10: A verification or reset code locks after 5 failed attempts; the lock window starts at 15 minutes, doubles per earlier lockout, and is capped at 24 hours; resends wait 5 minutes

- Attackers: A1
- Strength: proven, tested
- Checks:
  - `backend/src/domain/entities/attempt_lockout.rs::lock_window_is_between_15_minutes_and_24_hours`
  - `backend/src/domain/entities/attempt_lockout.rs::lock_window_never_shrinks_as_lockouts_grow`
  - `backend/src/domain/entities/attempt_lockout.rs::lock_window_doubles_per_earlier_lockout_up_to_24_hours`
  - `backend/src/domain/entities/attempt_lockout.rs::locked_inside_the_window_is_never_reissued`
  - `backend/src/domain/entities/attempt_lockout.rs::unlocked_inside_the_cooldown_is_refused`
  - `backend/src/domain/entities/attempt_lockout.rs::reissued_once_every_wait_has_passed`
  - `backend/src/domain/entities/attempt_lockout.rs::reported_waits_are_never_negative`
  - `backend/src/domain/entities/attempt_lockout.rs::a_lock_with_under_a_second_left_still_holds`
  - `backend/src/domain/entities/attempt_lockout.rs::lock_duration_doubles_per_earlier_lockout`
  - `backend/src/domain/entities/attempt_lockout.rs::lock_duration_is_capped`
  - `backend/src/domain/entities/attempt_lockout.rs::reissue_lock_outranks_cooldown`
  - `backend/src/domain/entities/attempt_lockout.rs::resend_cooldown`
  - `backend/tests/verification_integration.rs::verify_email_locked_code_returns_code_locked`
  - `backend/tests/verification_integration.rs::lock_window_doubles_after_each_lockout`

**Proven** (Kani, `just verify`) over every `i32` attempt count and lockout
count, and every send and current time from 2000 to 2200 to the microsecond,
in either order:

- the window is exactly 15 minutes doubled per earlier lockout (15, 30, 60 …
  960 minutes), then 24 hours from the 7th lockout on; it's always between 15
  minutes and 24 hours and never shrinks as lockouts grow
- a locked code is never reissued inside its window
- an unlocked code is never reissued inside the 5-minute cooldown
- a code is reissued once both waits have passed
- no reported wait is negative, and no code below the cap is reported locked

The proofs run on the integer policy in `attempt_lockout.rs`. The
`AttemptLockout` methods convert their `DateTime`s to microseconds and call
it; that conversion is covered by the unit tests above, not by proof. Proving
through chrono's calendar arithmetic directly didn't finish.

What's proven is the **decision**: given a code's stored state and the time,
whether a replacement may be issued. Whether the system acts on that decision
atomically is P-11 and P-12.

The proofs found one defect, now fixed: a wait under one second rounded down
to zero, so a locked code could be replaced up to a second before its window
ended.

### P-11: No path can reset an active lock by issuing a new code

- Attackers: A1
- Strength: modelled, tested
- Checks:
  - `models/attempt_claim.qnt::lockIsNeverLiftedInsideItsWindow`
  - `models/attempt_claim.qnt::noReissueInsideTheCooldown`
  - `backend/tests/model_conformance.rs::email_codes_follow_the_attempt_claim_model`
  - `backend/tests/model_conformance.rs::reset_codes_follow_the_attempt_claim_model`
  - `backend/tests/reissue_race_integration.rs::resend_racing_the_locking_guess_keeps_the_lock`
  - `backend/tests/reissue_race_integration.rs::reregister_racing_the_locking_guess_keeps_the_lock`
  - `backend/tests/reissue_race_integration.rs::reset_request_racing_the_locking_guess_keeps_the_lock`
  - `backend/tests/verification_integration.rs::resend_while_locked_issues_no_new_code`
  - `backend/tests/verification_integration.rs::reregistering_does_not_reset_a_locked_code`
  - `backend/tests/repository_integration.rs::password_reset_save_upsert_consume`
  - `backend/tests/repository_integration.rs::set_last_sent_at_moves_only_the_named_unlocked_code`
  - `backend/src/domain/entities/attempt_lockout.rs::lock_window_table_matches_the_policy`
  - `just models`

Each issuing path (`resend_verification`, `request_password_reset`,
`send_verification_code`) decides with `reissue` on the row it read, then
writes the new code with the repositories' `save`. A wrong guess can lock the
code between the two, or a concurrent request can issue one, so `save`
re-checks the whole reissue rule in the same statement, at the new code's send
time: its `ON CONFLICT … DO UPDATE … WHERE` refuses to replace a row that is at
the cap inside its lock window, or any row inside the resend cooldown, and the
use case answers as if it had seen that first. The SQL reads the windows from
`LOCK_WINDOWS_MINUTES`, which Kani proves equal to the policy for every
lockout count. Registration's cooldown release after a failed email moves the
send time of only the code it issued, and only below the cap, since the send
time also starts the lock window.

**Modelled** (Quint, `just models`): every interleaving of 3 guesses, 2
issuing requests (decide, write, deliver with a possibly failed email), expiry
cleanup and time passing, up to 8 steps with Apalache and 30 steps by random
search, from any one stored code (0–5 attempts, 0–2 earlier lockouts). The
`unguarded_upsert` module models the old upsert and must keep its
counterexample (a 4-step race); `lock_only_guard` models a guard without the
cooldown and must keep its own. Traces replay against both repositories on
separate connections, issuing codes through the production constructors. The
race tests drive the real endpoints: before the fix, 37 to 38 of 40 rounds per
path overwrote the lock. Bound: 3 guesses can't lock a freshly issued code, so
the model can't tell the release's cap condition apart; its repository test
covers it.

Found by OT-010 slice 3's review and fixed in slice 4. Before the fix, an
attacker who timed a resend with each 5th guess got about 5 guesses per
5-minute cooldown instead of the escalating lock.

### P-12: Concurrent guesses at one code get at most 5 comparisons

- Attackers: A1
- Strength: modelled, tested
- Checks:
  - `models/attempt_claim.qnt::atMostFiveWrongGuessesPerCode`
  - `models/attempt_claim.qnt::goneCodeIsNeverReportedLocked`
  - `backend/tests/model_conformance.rs::email_codes_follow_the_attempt_claim_model`
  - `backend/tests/model_conformance.rs::reset_codes_follow_the_attempt_claim_model`
  - `backend/tests/attempt_lockout_integration.rs::register_attempt_caps_under_concurrency`
  - `backend/tests/attempt_lockout_integration.rs::register_attempt_reports_not_found_without_row`
  - `backend/tests/attempt_lockout_integration.rs::claim_naming_a_replaced_code_finds_no_code`
  - `just models`
- Gap: Accepted (2026-11-16). The model treats each statement as atomic. The claim's "does the code still exist" check reads the statement's snapshot, so a guess whose claim waits on a concurrent replacement or deletion of its code can still be told "locked" rather than "no code". The message is the only effect: no slot is taken and nothing is compared. Fixing it would need a stricter isolation level on every guess, for a wording difference.

**Modelled** with the same bounds as P-11: at most 5 wrong guesses are
compared against any one code (a correct guess releases its slot), and a guess
whose code was verified, cleaned up or replaced gets the no-code answer, never
"too many attempts".

The model found a defect, fixed in OT-010 slice 4: the attempt claim named
only the user. A guess that read a code which was then replaced claimed a slot
on the new code but was compared against the old one, past the old code's
cap, and could be told "locked" about a code it never saw. The claim and its
release now name the code id the guess read. The `claim_by_user` module models
the old claim and must keep its counterexample (5 steps).

### P-13: The failed-attempt counter can't wrap round to "unlocked"

- Attackers: A1
- Strength: tested
- Checks:
  - `backend/tests/attempt_lockout_integration.rs::register_attempt_caps_under_concurrency`

The counter is incremented only in SQL, by the repositories' attempt claim
(`SET attempts = attempts + 1 … WHERE attempts < $2`). It stops at the cap of
5, so it can't approach overflow, and the concurrency test above exercises
that guard. Slice 3's Kani proof found that the domain's own
`increment_attempts` could wrap at `i32::MAX`. It had no production caller, so
it was deleted rather than kept as a second counter that doesn't matter.

### Sessions and refresh tokens

### P-14: A refresh token yields at most one new session, even under concurrent use

- Attackers: A3
- Strength: modelled, tested
- Checks:
  - `models/refresh_rotation.qnt::atMostOneSuccessorPerToken`
  - `backend/tests/model_conformance.rs::refresh_tokens_follow_the_rotation_model`
  - `backend/tests/refresh_token_integration.rs::concurrent_rotate_lets_exactly_one_winner`
  - `backend/src/application/use_cases/auth/refresh_token.rs::rotates_token_on_valid_refresh`
  - `just models`

**Modelled** (Quint, `just models`): every interleaving of 3 refresh requests
presenting any of the session's tokens, a sign-out, expiry cleanup and time
passing, up to 8 steps with Apalache and 30 by random search. Rotation's
revoke-and-insert transaction is one step, which holds because its
`WHERE revoked_at IS NULL` re-checks the row after a concurrent writer
commits. Traces replay against the real repository on separate connections.
Logins aren't modelled: they mint unrelated tokens.

### P-15: Presenting a revoked refresh token revokes every session of that Member, even if it has also expired

- Attackers: A3
- Strength: modelled, tested
- Checks:
  - `models/refresh_rotation.qnt::reuseAfterRotationIsDetected`
  - `models/refresh_rotation.qnt::reuseRevokesEverySession`
  - `backend/tests/model_conformance.rs::refresh_tokens_follow_the_rotation_model`
  - `backend/src/application/use_cases/auth/refresh_token.rs::revoked_reuse_revokes_all_even_when_also_expired`
  - `backend/src/domain/entities/refresh_token.rs::test_status_revoked_outranks_expired`
  - `backend/src/domain/entities/refresh_token.rs::test_revoke_is_idempotent`
  - `backend/tests/auth_properties_integration.rs::reusing_a_rotated_refresh_token_ends_every_session`
  - `models/auth_protocols.spthy::stolen_reuse_ends_every_session`
  - `just models`
  - `just protocols`
- Gap: Accepted (2026-11-16). This holds only while the token is still stored. Once it has expired and the cleanup job has deleted it, presenting it is refused as an unknown token, not treated as reuse, so the Member's later sessions are not revoked. It can't yield a session either way. The model shows this (`everyReplayOfARotatedTokenIsDetected`, not part of `safety`). Keeping revoked tokens past expiry would close it. It's accepted because the stolen token can't produce a session, and the replay window it gives up only opens after the token's 30-day expiry.

**Modelled** with the P-14 bounds: a request that presents an already rotated
token, while it's stored, is treated as reuse, and once reuse is detected no
session of that Member is left live. A request that read the token before a
concurrent rotation committed gets a conflict instead; the next presentation
is the reuse.

**Modelled** (Tamarin, `just protocols`; attacker and assumptions under
[Protocol model](#protocol-model)). Once a rotated token is presented again,
by anyone and with any fingerprint, no token the Member held before that is
accepted again. This lemma holds by construction: the model's revoke-all is a
restriction that says what the code's `UPDATE … WHERE user_id = $1` does. What
it adds is that the attacker has no way round reuse detection; the evidence
that the code revokes is the integration test the rule names, and the Quint
replay. A token deleted by expiry cleanup is outside the lemma, as the gap
says.

### P-16: A refresh token works only with the device fingerprint it was issued to

- Attackers: A3
- Strength: modelled, tested
- Checks:
  - `models/auth_protocols.spthy::refresh_needs_the_fingerprint`
  - `models/auth_protocols.spthy::fingerprint_known_only_if_stolen`
  - `backend/tests/auth_properties_integration.rs::a_refresh_token_is_refused_from_another_device`
  - `backend/src/application/use_cases/auth/refresh_token.rs::rejects_device_fingerprint_mismatch`
  - `backend/src/domain/entities/refresh_token.rs::test_validate_fingerprint_mismatch`
  - `just protocols`

**Modelled** (Tamarin, `just protocols`; attacker and assumptions under
[Protocol model](#protocol-model)). A Member's refresh token works on another
client only if the fingerprint it's bound to was stolen too, and the
fingerprint only ever travels inside TLS.

### P-33: A session or refresh token reaches only the Member who signed in, and a replayed sign-in, refresh or Google callback gains nothing

- Attackers: A1, A3, A4
- Strength: modelled, tested
- Checks:
  - `models/auth_protocols.spthy::browser_tokens_stay_secret`
  - `models/auth_protocols.spthy::replayed_sign_in_gains_nothing`
  - `models/auth_protocols.spthy::refresh_replay_gains_nothing`
  - `backend/tests/auth_properties_integration.rs::reusing_a_rotated_refresh_token_ends_every_session`
  - `backend/tests/refresh_token_integration.rs::concurrent_rotate_lets_exactly_one_winner`
  - `just protocols`

**Modelled** (Tamarin, `just protocols`; attacker and assumptions under
[Protocol model](#protocol-model)). Tokens issued to a browser are never known
to the attacker unless stolen from it. The attacker's own client gets a
Member's session only with the Member's password, or by refreshing, which
needs a token and fingerprint both stolen (P-16). A refresh token yields one
successor, so replaying it gains nothing; that lemma holds by construction
from the model's rotate-once restriction (rotation's `WHERE revoked_at IS
NULL`), which the linked rotation tests show the code makes. With TLS removed
(`-D=INSECURE_CHANNEL`), both sign-in lemmas are attacked, so they rest on the
channel.

### P-17: Signing out revokes the refresh token and clears the session cookie

- Attackers: A3
- Strength: tested
- Checks:
  - `backend/tests/profile_integration.rs::logout_clears_session_cookie`
  - `backend/src/application/use_cases/auth/revoke_token.rs::test_revoke_existing_token`
  - `backend/tests/auth_properties_integration.rs::a_refresh_token_is_refused_after_logout`
- Gap: If revoking the refresh token fails (a database error), logout still clears the cookies and reports success, and the token stays valid until it expires or rotates. Logout logs a warning when this happens, but it doesn't guarantee revocation. The impact is low, since the token was in an `HttpOnly` cookie that has just been cleared.

### P-18: Retiring a signing key ends its sessions and links, and rotating keeps them

- Attackers: A3
- Strength: tested
- Checks:
  - `backend/tests/token_keys_integration.rs::tokens_from_the_previous_key_keep_working_after_rotation`
  - `backend/tests/token_keys_integration.rs::retiring_a_key_signs_its_sessions_in_again_silently`
  - `backend/tests/token_keys_integration.rs::retiring_a_key_invalidates_its_reset_links`

### P-19: One-time codes and magic links are accepted only before they expire and while unlocked

- Attackers: A1, A3
- Strength: tested
- Checks:
  - `backend/tests/verification_integration.rs::verify_email_expired_code_returns_code_expired`
  - `backend/tests/verification_integration.rs::magic_link_expired_token_is_rejected`
  - `backend/src/domain/entities/password_reset_code.rs::test_expired_code`
  - `backend/src/domain/entities/email_verification_code.rs::test_is_magic_token_expired`
- Gap: Sampled, not proven. Kani proof planned (OT-010).

### Cross-site

### P-20: A state-changing request on an authenticated route needs a CSRF token that matches its cookie and is bound to the session's user

- Attackers: A1, A4
- Strength: modelled, tested, property-tested
- Checks:
  - `backend/tests/security_integration.rs::csrf_header_cookie_mismatch_rejected`
  - `backend/tests/security_integration.rs::csrf_cookie_without_header_rejected`
  - `backend/tests/security_integration.rs::csrf_token_bound_to_other_user_rejected`
  - `backend/src/infrastructure/api/csrf.rs::test_verify_rejects_other_user`
  - `backend/src/infrastructure/api/csrf.rs::prop_arbitrary_strings_fail`
  - `models/auth_protocols.spthy::cross_site_pages_cannot_change_state`
  - `models/auth_protocols.spthy::cross_site_changes_need_a_token_signed_for_the_member`
  - `just protocols`

**Modelled** (Tamarin, `just protocols`; attacker and assumptions under
[Protocol model](#protocol-model)). For this property the modelled browser is
the worst case: it attaches every cookie to a cross-site request, as if
`SameSite` didn't apply, so only the double-submit check stands in the way. A
change accepted from a browser was made by the app's own script, unless that
browser's tokens were stolen or the Member's password leaked. The user binding
is what would hold if the attacker could also write the app's cookies
(`-D=NO_HOST_PREFIX`): a cross-site change still needs a token signed for that
Member, and without the binding it is attacked.

### P-21: Pre-auth routes that skip CSRF can't be driven cross-site

Login, sign-up, refresh, logout, the code routes and the device-fingerprint
route are excluded from CSRF. Each relies on something a cross-site form can't
do:

- **Routes with a body** accept only `application/json`. A form can send
  `text/plain`, form-encoded or multipart bodies without a CORS preflight; each
  gets the standard JSON 400.
- **Refresh** requires the `x-device-fingerprint` header, which a form can't
  set.
- **CORS** never grants another origin, so a script on another site can't read
  a response or send JSON.

- Attackers: A1
- Strength: modelled, tested
- Checks:
  - `models/auth_protocols.spthy::exempt_routes_cannot_be_driven_cross_site`
  - `just protocols`
  - `backend/tests/auth_properties_integration.rs::csrf_exempt_routes_cannot_be_driven_cross_site`
  - `backend/tests/auth_properties_integration.rs::cors_grants_no_other_origin`
  - `backend/src/infrastructure/api/middleware/csrf.rs::test_excluded_paths`
- Gap: Accepted. Logout takes no body and no custom header. A cross-site form can POST to it, and what stops it ending the session is the browser not sending or setting `SameSite=Lax` cookies on a cross-site POST (P-24). A server test can't observe that browser behaviour, and the model leaves logout out. The impact would be a forced sign-out, not access.

**Modelled** (Tamarin, `just protocols`; attacker and assumptions under
[Protocol model](#protocol-model)). Here the modelled browser follows
`SameSite=Lax` and sends a cross-site request with only a simple body
(form-encoded, multipart or `text/plain`), no custom header and no cookies. No
exempt route acts on such a request; if a route accepted a simple body
(`-D=ACCEPT_SIMPLE_BODIES`), it would be attacked.

### P-22: Google sign-in completes only in the browser that started it

- Attackers: A1, A4
- Strength: modelled, tested
- Checks:
  - `models/auth_protocols.spthy::google_sign_in_completes_where_it_started`
  - `models/auth_protocols.spthy::no_login_csrf`
  - `just protocols`
  - `backend/tests/oauth_integration.rs::callback_without_state_cookie_is_rejected`
  - `backend/tests/oauth_integration.rs::callback_with_another_flows_nonce_is_rejected`
  - `backend/tests/oauth_integration.rs::callback_with_forged_state_is_rejected`
  - `backend/tests/oauth_integration.rs::callback_for_different_provider_than_state_is_rejected`
  - `backend/src/infrastructure/api/oauth_state.rs::tampered_oauth_state_is_refused`
  - `backend/src/infrastructure/api/oauth_state.rs::expired_oauth_state_is_refused`
  - `backend/tests/oauth_integration.rs::auth_cookies_are_host_only`

**Modelled** (Tamarin, `just protocols`; attacker and assumptions under
[Protocol model](#protocol-model)). A Google sign-in completes in a browser
only if that browser started the flow, and a Member's browser is only ever
signed in to that Member's own account. Without the callback's check that the
state's nonce equals the `oauth_state` cookie (`-D=NO_STATE_CHECK`), both are
attacked with the login-CSRF trace. The model found that both also depend on
the attacker not writing the app's cookies: before every auth cookie got the
`__Host-` prefix (`-D=NO_HOST_PREFIX`), an attacker on a sibling host, or on
plain HTTP before HSTS, could plant their own flow's `oauth_state` nonce, or
their own session cookies, and sign the victim in to the attacker's account.
Both are still attacked in that variant, and `auth_cookies_are_host_only`
shows the code sets every cookie with the prefix.

### P-23: Linking a Google identity can't take over another account

- Attackers: A1, A2
- Strength: tested
- Checks:
  - `backend/tests/oauth_integration.rs::link_identity_owned_by_another_user_redirects_already_linked`
  - `backend/tests/oauth_integration.rs::link_with_unverified_provider_email_is_refused`
  - `backend/tests/oauth_integration.rs::login_with_unverified_account_email_redirects_with_conflict`
  - `backend/tests/oauth_integration.rs::login_with_unverified_provider_email_creates_no_account`
  - `backend/tests/oauth_integration.rs::verify_email_cannot_take_over_oauth_only_account`

### P-24: Auth cookies are `__Host-` prefixed (so only this host, over HTTPS, can set them), `Secure`, `Path=/`, `SameSite=Lax`, and `HttpOnly` except the CSRF cookie

- Attackers: A1, A4
- Strength: tested
- Checks:
  - `backend/src/infrastructure/api/cookies.rs::every_cookie_meets_the_host_prefix_rules`
  - `backend/src/infrastructure/api/cookies.rs::only_the_csrf_cookie_is_readable_by_scripts`
  - `backend/src/infrastructure/api/cookies.rs::extract_cookie_ignores_the_unprefixed_name`
  - `backend/tests/oauth_integration.rs::auth_cookies_are_host_only`
  - `apps/web/tests/integration/signup.spec.ts`

`Path=/` is what the prefix requires, so the session and refresh cookies are
now sent on every same-origin request, not just to `/api`. They stay
`HttpOnly`, and neither edge logs `Cookie` headers by default (Caddy redacts
them; Traefik drops request headers). The real-backend browser flow checks a
browser keeps and sends the prefixed cookies on `http://localhost`, which
browsers treat as secure; Safari may need HTTPS for local development.

### Confidentiality

### P-25: Telemetry never holds passwords, emails, tokens or codes

- Attackers: A3
- Strength: tested
- Checks:
  - `backend/tests/telemetry_integration.rs::auth_flows_leave_no_secrets_in_telemetry`
  - `backend/src/infrastructure/crypto/keyring.rs::startup_log_names_the_keys_and_holds_no_secrets`

### P-26: Internal error details reach clients only in DEV_MODE

- Attackers: A1
- Strength: tested
- Checks:
  - `backend/src/infrastructure/api/error.rs::test_into_response_internal_redacted_in_prod`
  - `backend/src/infrastructure/api/error.rs::test_into_response_database_redacted_in_prod`

### P-27: A database copy gives no usable credentials

Passwords are argon2 hashes, and codes and refresh tokens are stored as
hashes. Access, signup and reset tokens are signed rather than stored. Google
linking keeps only the provider's user id and email, and no provider token is
stored. (The secrets cipher in `crypto.rs` exists, but nothing calls it yet. If
a stored secret is ever added, it should be encrypted with it and added to this
test.)

- Attackers: A5
- Strength: tested
- Checks:
  - `backend/tests/auth_properties_integration.rs::a_database_copy_holds_no_usable_credentials`

The test runs sign-up, verification, sign-in, refresh and a password reset.
It dumps every row of every table while each code is still live (using a code
deletes its row), and again at the end. It checks that none of the passwords,
codes, magic-link token or other tokens those flows handled appears in any
dump.

### Account lifecycle

### P-28: A deleted account stays deleted, and a pending deletion is purged only after its date

- Attackers: A2
- Strength: tested
- Checks:
  - `backend/src/domain/entities/user.rs::test_deleted_is_terminal`
  - `backend/src/domain/entities/user.rs::test_schedule_deletion_is_idempotent`
  - `backend/src/domain/entities/user.rs::test_schedule_deletion_sets_future_timestamp`
  - `backend/src/domain/entities/user.rs::test_is_ready_for_deletion_when_scheduled_in_past`
  - `backend/tests/account_deletion_integration.rs::deleted_account_cannot_be_pending_deletion`
  - `backend/tests/account_deletion_integration.rs::anonymize_skips_account_cancelled_after_the_scan`
- Gap: Sampled, not proven for every state and time. Kani proof planned (OT-010).

### P-29: Deleting an account needs the current password

- Attackers: A3
- Strength: tested
- Checks:
  - `backend/tests/profile_integration.rs::request_deletion_wrong_password_is_rejected`
  - `backend/tests/profile_integration.rs::request_deletion_without_confirmation_returns_400`

### P-30: Addresses differing only in case or surrounding whitespace are one identity

- Attackers: A1
- Strength: property-tested
- Checks:
  - `backend/src/domain/value_objects/email.rs::prop_email_normalized_to_lowercase`
  - `backend/src/domain/value_objects/email.rs::prop_email_whitespace_trimmed`
- Gap: Idempotence isn't checked. Kani proof for short ASCII planned (OT-010).

### Robustness

### P-31: Garbage in any token or cookie is refused cleanly, never with a 500 or as a valid session

- Attackers: A1
- Strength: tested, property-tested
- Checks:
  - `backend/tests/profile_integration.rs::me_with_garbage_token_returns_401`
  - `backend/src/infrastructure/crypto/keyring.rs::refuses_malformed_tokens`
  - `backend/src/infrastructure/crypto/keyring.rs::prop_random_segments_never_verify`
  - `backend/src/infrastructure/api/cookies.rs::extract_cookie_returns_none_for_non_utf8_header`
  - `backend/tests/oauth_integration.rs::callback_with_malformed_user_id_in_state_is_rejected`
- Gap: Sampled. Fuzzing every operation is OT-011 (API fuzzing); link its checks here when it lands.

### P-32: Malformed bodies, queries and paths get the JSON 400 before any handler logic

- Attackers: A1
- Strength: tested
- Checks:
  - `backend/tests/api_contract_integration.rs::malformed_json_body_gets_standard_error`
  - `backend/tests/api_contract_integration.rs::malformed_query_parameter_gets_standard_error`
  - `backend/tests/api_contract_integration.rs::malformed_path_parameter_gets_standard_error`
- Gap: Sampled. Fuzzing every operation is OT-011 (API fuzzing); link its checks here when it lands.

## Spec coverage

Which properties carry each requirement of the `security` spec added by
OT-010:

| Requirement                                               | Properties                               |
| --------------------------------------------------------- | ---------------------------------------- |
| Security properties are written down and each has a check | this document, `just threat-model-check` |
| Every route is either public by name or guarded           | P-06, P-07                               |
| A Member can reach only their own account                 | P-09                                     |
| Code-entry lockout holds for every input                  | P-10, P-11, P-13                         |
| One-time codes are accepted only while live               | P-19                                     |
| A revoked refresh token is never accepted                 | P-15                                     |
| Role management never escalates privilege                 | P-08                                     |
| A deleted account stays deleted                           | P-28                                     |
| Email addresses normalise to one identity                 | P-30                                     |
| Concurrent requests cannot get around auth limits         | P-11, P-12, P-14, P-15                   |
| The sign-in protocols resist an active network attacker   | P-15, P-16, P-20, P-21, P-22, P-33       |
| Auth guarantees are machine-checked on every change       | the Strength line of every property      |

## Adding a property or a check

1. Add a `### P-<nn>: <statement>` heading under the right section. Numbers
   are never reused.
2. List checks under `- Checks:` as backticked entries, one per line:
   - `path/to/file.rs::function_name` for a test or Kani harness
   - `path/to/model.qnt::name` or `path/to/model.spthy::name` for a model
     invariant or lemma
   - `path/to/file` for a whole-file check, such as a fuzz target
   - `just <recipe>` for a gate
3. If coverage is missing or weaker than the statement, add a `- Gap:` line
   saying what is missing and where it's planned.
4. In a Tamarin model, a rule that encodes a check the code must make names
   the integration test that shows the code makes it, in a comment block right
   above the rule (`// Checks: backend/tests/<file>.rs::<fn>`). The gate
   resolves those too.
5. Run `just threat-model-check`.
