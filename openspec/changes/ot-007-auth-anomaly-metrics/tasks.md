# Tasks

## 1. Slice 1 (first slice): sign-in and code lockouts, counted and alerted

Delivers these scenarios: Failed sign-in is counted; Unknown email counts the same
as a wrong password; Successful sign-in is counted; A lockout is counted once;
Expired code is not counted as a guess (email and reset codes); Measurements
hold no personal data; Measuring never affects sign-in; No traffic yet;
Credential stuffing burst; A forgetful Member does not alert; Code guessing
across many accounts; Normal use stays quiet; Local development never sends
email.

- [ ] 1.1 Add `AuthMetrics` to `infrastructure/telemetry/metrics.rs`, with the `auth.attempts` and `auth.code.lockouts` counters and closed label enums (`AuthFlow`, `AuthOutcome`) (D3). Unit tests with the in-memory exporter check the names, labels and that series don't grow with the number of calls. Verify with `just test`.
- [ ] 1.2 Add typed `ResetCodeError` to `ValidateResetCodeUseCase`, keeping `attempts_remaining` from `register_attempt` (D2). Its `DomainError` mapping reproduces today's messages. Use-case tests cover each variant, and a handler test asserts today's status and body for each.
- [ ] 1.3 Hold `AuthMetrics` in the auth and verification handler deps (built in `main.rs`). Record sign-in outcomes in `login`, and email-code and reset-code outcomes plus lockouts in `verify_email` and `verify_reset_code` (D1, D3). Handler tests with an in-memory meter cover: wrong password, unknown email, success, 5 wrong codes then 3 locked (5 failure, 1 lockout, 3 locked), expired.
- [ ] 1.4 Add a test that runs sign-in and code flows for several users and asserts that no auth metric label value contains an email, user ID, code or token. Verify with `just test`.
- [ ] 1.5 Add the `auth` vmalert group with `AuthSignInFailuresHigh` and `AuthCodeLockoutsHigh` (D5), and unit tests in `deploy/observability/alerts/tests/` for: just under and just over each threshold, the 6-failures-then-reset Member, and resolution. Verify with `just obs-check`.
- [ ] 1.6 Add the `auth-security` Grafana dashboard with the sign-in and code panels and threshold lines (D4). Link alert annotations to its panels. Check locally with `just obs-up` that the panels show "no data", not errors, on a fresh stack.
- [ ] 1.7 Add `loadtest/scenarios/auth-abuse.js` (wrong passwords against seeded accounts, wrong codes until locked) (D6). Run it against the local full stack, and confirm in the local alert view that both alerts fire and no email is sent.
- [ ] 1.8 Run `just load auth-mix` and confirm that no auth alert fires and the dashboard shows only successes. Record both results in design.md Hypothesis.
- [ ] 1.9 With the observability stack stopped, sign in and verify a code. Confirm that responses are unchanged and that a warning log says telemetry isn't delivered.
- [ ] 1.10 `just api-types-check` shows no diff, `just ci` is green. Note for the first deploy: restart vmalert and Grafana for the rules and dashboard, then roll the backend digest and `just smoke <url>`.

## 2. Feature done

- [ ] 2.1 After slice 1 ships, revise design.md with what it showed, then add the remaining slice to this file: magic-link outcomes, bot-check rejections, session anomalies (typed `RefreshError`), and their alerts and panels.
- [ ] 2.2 Document the auth metrics, the dashboard and the alert thresholds in `deploy/compose/README.md` (Observability), including the manual Turnstile always-fail check. Fix the `login_user.rs` doc comment and the README so they no longer say Traefik rate limiting exists (it is planned for the production edge, not built).
- [ ] 2.3 Archive the change. Thresholds are revisited after the first month in production (recorded in Hypothesis).
