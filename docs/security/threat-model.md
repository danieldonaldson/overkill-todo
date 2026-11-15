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

1. **Browser ↔ edge**: TLS; cookies `HttpOnly; Secure; SameSite=Lax`; CORS
   allows only the frontend origin.
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

### P-04: A session issued before a password reset, account purge or role change is refused on account-sensitive and admin routes

- Attackers: A3
- Strength: tested
- Checks:
  - `backend/src/infrastructure/api/middleware/auth.rs::changed_role_invalidates_session`
  - `backend/src/infrastructure/api/middleware/auth.rs::token_from_cutoff_second_remains_revoked_after_role_cycle`
  - `backend/tests/admin_integration.rs::demoted_admin_token_is_rejected`
  - `backend/src/application/use_cases/auth/reset_password.rs::test_reset_password_success`
  - `backend/src/application/use_cases/user/change_password.rs::test_change_password_without_refresh_token_revokes_all`
- Gap: The password-reset cutoff is checked only with mocks: the use case records it, and the middleware honours a cutoff. No integration test signs in, resets the password, and shows that the old access token is refused on a sensitive route.

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
  - `backend/tests/profile_integration.rs::export_data_returns_user_profile`
- Gap: The export test checks only the caller's own fields with a single account in the database. Profile handlers take the account from the session, and none accept an account id. That is true by construction today, but no test asserts that an id, email or token naming another account in the path, query or body is ignored, or that an export holds only the caller's data. Planned (OT-010, authorisation slice).

### Brute force on one-time codes

### P-10: A verification or reset code locks after 5 failed attempts; the lock window starts at 15 minutes, doubles per earlier lockout, and is capped at 24 hours; resends wait 5 minutes

- Attackers: A1
- Strength: tested
- Checks:
  - `backend/src/domain/entities/attempt_lockout.rs::lock_duration_doubles_per_earlier_lockout`
  - `backend/src/domain/entities/attempt_lockout.rs::lock_duration_is_capped`
  - `backend/src/domain/entities/attempt_lockout.rs::reissue_lock_outranks_cooldown`
  - `backend/src/domain/entities/attempt_lockout.rs::resend_cooldown`
  - `backend/tests/verification_integration.rs::verify_email_locked_code_returns_code_locked`
  - `backend/tests/verification_integration.rs::lock_window_doubles_after_each_lockout`
- Gap: Sampled at a few points, not proven for every attempt count, lockout count and time. Kani proof planned (OT-010).

### P-11: No path can reset an active lock by issuing a new code

- Attackers: A1
- Strength: tested
- Checks:
  - `backend/tests/verification_integration.rs::resend_while_locked_issues_no_new_code`
  - `backend/tests/verification_integration.rs::reregistering_does_not_reset_a_locked_code`

### P-12: Concurrent guesses at one code get at most 5 comparisons

- Attackers: A1
- Strength: tested
- Checks:
  - `backend/tests/attempt_lockout_integration.rs::register_attempt_caps_under_concurrency`
  - `backend/tests/attempt_lockout_integration.rs::register_attempt_reports_not_found_without_row`
- Gap: One concurrent run is sampled, not every interleaving. Quint model planned (OT-010).

### P-13: The failed-attempt counter can't wrap round to "unlocked"

- Attackers: A1
- Strength: none
- Gap: Known defect. `increment_attempts` is a plain `i32` increment, and release builds don't check overflow, so it would wrap to negative and read as unlocked. It's unreachable while the database caps attempts at 5. Fixed with a proof in OT-010 (Kani slice).

### Sessions and refresh tokens

### P-14: A refresh token yields at most one new session, even under concurrent use

- Attackers: A3
- Strength: tested
- Checks:
  - `backend/tests/refresh_token_integration.rs::concurrent_rotate_lets_exactly_one_winner`
  - `backend/src/application/use_cases/auth/refresh_token.rs::rotates_token_on_valid_refresh`
- Gap: One concurrent run is sampled, not every interleaving. Quint model planned (OT-010).

### P-15: Presenting a revoked refresh token revokes every session of that Member, even if it has also expired

- Attackers: A3
- Strength: tested
- Checks:
  - `backend/src/application/use_cases/auth/refresh_token.rs::revoked_reuse_revokes_all_even_when_also_expired`
  - `backend/src/domain/entities/refresh_token.rs::test_status_revoked_outranks_expired`
  - `backend/src/domain/entities/refresh_token.rs::test_revoke_is_idempotent`

### P-16: A refresh token works only with the device fingerprint it was issued to

- Attackers: A3
- Strength: tested
- Checks:
  - `backend/src/application/use_cases/auth/refresh_token.rs::rejects_device_fingerprint_mismatch`
  - `backend/src/domain/entities/refresh_token.rs::test_validate_fingerprint_mismatch`

### P-17: Signing out revokes the refresh token and clears the session cookie

- Attackers: A3
- Strength: tested
- Checks:
  - `backend/tests/profile_integration.rs::logout_clears_session_cookie`
  - `backend/src/application/use_cases/auth/revoke_token.rs::test_revoke_existing_token`
- Gap: No end-to-end test shows that a refresh token presented after logout is refused.

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
- Strength: tested, property-tested
- Checks:
  - `backend/tests/security_integration.rs::csrf_header_cookie_mismatch_rejected`
  - `backend/tests/security_integration.rs::csrf_cookie_without_header_rejected`
  - `backend/tests/security_integration.rs::csrf_token_bound_to_other_user_rejected`
  - `backend/src/infrastructure/api/csrf.rs::test_verify_rejects_other_user`
  - `backend/src/infrastructure/api/csrf.rs::prop_arbitrary_strings_fail`
- Gap: The CSRF defence hasn't been modelled against an attacker who controls a site. Tamarin model planned (OT-010).

### P-21: Pre-auth routes that skip CSRF can't be driven cross-site

Login, sign-up, refresh, logout and the code routes are excluded from CSRF.
They rely on accepting only `application/json` bodies (which a cross-site form
can't send without a CORS preflight) and on CORS allowing only the frontend
origin.

- Attackers: A1
- Strength: none
- Gap: `test_excluded_paths` pins which routes skip CSRF, but nothing tests the defence they rely on. No test shows that a form-encoded or `text/plain` POST to an excluded route is refused, or that CORS refuses another origin. A cross-site logout would succeed if those defences slipped. Planned (OT-010, protocol slice).

### P-22: Google sign-in completes only in the browser that started it

- Attackers: A1, A4
- Strength: tested
- Checks:
  - `backend/tests/oauth_integration.rs::callback_without_state_cookie_is_rejected`
  - `backend/tests/oauth_integration.rs::callback_with_another_flows_nonce_is_rejected`
  - `backend/tests/oauth_integration.rs::callback_with_forged_state_is_rejected`
  - `backend/tests/oauth_integration.rs::callback_for_different_provider_than_state_is_rejected`
  - `backend/src/infrastructure/api/oauth_state.rs::tampered_oauth_state_is_refused`
  - `backend/src/infrastructure/api/oauth_state.rs::expired_oauth_state_is_refused`
- Gap: Tested message by message, but not proven against an active attacker. Tamarin model planned (OT-010).

### P-23: Linking a Google identity can't take over another account

- Attackers: A1, A2
- Strength: tested
- Checks:
  - `backend/tests/oauth_integration.rs::link_identity_owned_by_another_user_redirects_already_linked`
  - `backend/tests/oauth_integration.rs::link_with_unverified_provider_email_is_refused`
  - `backend/tests/oauth_integration.rs::login_with_unverified_account_email_redirects_with_conflict`
  - `backend/tests/oauth_integration.rs::login_with_unverified_provider_email_creates_no_account`
  - `backend/tests/oauth_integration.rs::verify_email_cannot_take_over_oauth_only_account`

### P-24: Auth cookies are `HttpOnly` (except CSRF), `Secure`, `SameSite=Lax` and scoped to the narrowest path

- Attackers: A1, A4
- Strength: tested
- Checks:
  - `backend/src/infrastructure/api/cookies.rs::jwt_cookie_has_expected_attributes`
  - `backend/src/infrastructure/api/cookies.rs::refresh_cookie_is_scoped_and_secure`
  - `backend/src/infrastructure/api/cookies.rs::csrf_cookie_is_readable_by_js_but_secure`
  - `backend/src/infrastructure/api/cookies.rs::oauth_state_cookie_is_scoped_http_only_and_short_lived`

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

Passwords are argon2 hashes, codes and refresh tokens are stored hashed, and
OAuth secrets are encrypted under a key held outside the database.

- Attackers: A5
- Strength: tested
- Checks:
  - `backend/src/infrastructure/crypto.rs::secret_cipher_rejects_plaintext_and_tampering`
  - `backend/src/infrastructure/crypto.rs::secret_cipher_uses_fresh_nonce_each_call`
- Gap: No test reads the stored rows after sign-up, sign-in and reset and checks that no code, token or password appears in plain text.

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
- Gap: Sampled. Fuzzing planned (OT-010).

### P-32: Malformed bodies, queries and paths get the JSON 400 before any handler logic

- Attackers: A1
- Strength: tested
- Checks:
  - `backend/tests/api_contract_integration.rs::malformed_json_body_gets_standard_error`
  - `backend/tests/api_contract_integration.rs::malformed_query_parameter_gets_standard_error`
  - `backend/tests/api_contract_integration.rs::malformed_path_parameter_gets_standard_error`
- Gap: Sampled. Fuzzing planned (OT-010).

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
| Concurrent requests cannot get around auth limits         | P-12, P-14                               |
| The sign-in protocols resist an active network attacker   | P-15, P-16, P-20, P-21, P-22             |
| Malformed input never crashes or gets past a check        | P-31, P-32                               |
| Auth guarantees are machine-checked on every change       | the Strength line of every property      |

## Adding a property or a check

1. Add a `### P-<nn>: <statement>` heading under the right section. Numbers
   are never reused.
2. List checks under `- Checks:` as backticked entries, one per line:
   - `path/to/file.rs::function_name` for a test or Kani harness
   - `path/to/model.qnt::name` for a model invariant or lemma
   - `path/to/file` for a whole-file check, such as a fuzz target
   - `just <recipe>` for a gate
3. If coverage is missing or weaker than the statement, add a `- Gap:` line
   saying what is missing and where it's planned.
4. Run `just threat-model-check`.
