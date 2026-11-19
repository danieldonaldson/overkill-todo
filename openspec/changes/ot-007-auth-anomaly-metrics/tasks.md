# Tasks

## 1. Slice 1 (first slice): sign-in and code lockouts, counted and alerted

Delivers these scenarios: Failed sign-in is counted; Unknown email counts the same
as a wrong password; Successful sign-in is counted; A lockout is counted once;
Expired code is not counted as a guess (email and reset codes); Measurements
hold no personal data; Measuring never affects sign-in; No traffic yet;
Credential stuffing burst; A forgetful Member does not alert; Code guessing
across many accounts; Normal use stays quiet; Local development never sends
email.

- [x] 1.1 Add `AuthMetrics` to `infrastructure/telemetry/metrics.rs`, with the `auth.attempts` and `auth.code.lockouts` counters and closed label enums (`AuthFlow`, `AuthOutcome`) (D3). Unit tests with the in-memory exporter check the names, labels and that series don't grow with the number of calls. Verify with `just test`.
- [x] 1.2 Add typed `ResetCodeError` to `ValidateResetCodeUseCase`, keeping `attempts_remaining` from `register_attempt` (D2). Its `DomainError` mapping reproduces today's messages. Use-case tests cover each variant, and a handler test asserts today's status and body for each.
- [x] 1.3 Hold `AuthMetrics` in the auth and verification handler deps (built in `main.rs`). Record sign-in outcomes in `login`, and email-code and reset-code outcomes plus lockouts in `verify_email` and `verify_reset_code` (D1, D3). Handler tests with an in-memory meter cover: wrong password, unknown email, success, 5 wrong codes then 3 locked (5 failure, 1 lockout, 3 locked), expired.
- [x] 1.4 Add a test that runs sign-in and code flows for several users and asserts that no auth metric label value contains an email, user ID, code or token. Verify with `just test`.
- [x] 1.5 Add the `auth` vmalert group with `AuthSignInFailuresHigh` and `AuthCodeLockoutsHigh` (D5), and unit tests in `deploy/observability/alerts/tests/` for: just under and just over each threshold, the 6-failures-then-reset Member, and resolution. Verify with `just obs-check`.
- [x] 1.6 Add the `auth-security` Grafana dashboard with the sign-in and code panels and threshold lines (D4). Link alert annotations to its panels. Check locally with `just obs-up` that the panels show "no data", not errors, on a fresh stack.
- [x] 1.7 Add `loadtest/scenarios/auth-abuse.js` (wrong passwords against seeded accounts, wrong codes until locked) (D6). Run it against the local full stack, and confirm in the local alert view that both alerts fire and no email is sent.
- [x] 1.8 Run `just load auth-mix` and confirm that no auth alert fires and the dashboard shows only successes. Record both results in design.md Hypothesis.
- [x] 1.9 With the observability stack stopped, sign in and verify a code. Confirm that responses are unchanged and that a warning log says telemetry isn't delivered.
- [x] 1.10 `just api-types-check` shows no diff, `just ci` is green. Note for the first deploy: restart vmalert and Grafana for the rules and dashboard, then roll the backend digest and `just smoke <url>`.
  - _Done without a deploy: there is no production instance yet._ `just ci` green on 2026-11-17, with no OpenAPI or schema diff (`api-types-check`). The first-deploy steps are in `deploy/compose/README.md` (First deploy).

## 2. Slice 2: investigation context for alerts

Delivers these scenarios: Lockout names the account; Targeted guessing is
visible; Stuffing versus brute force; Unknown email names no account;
Investigation logs hold no secrets; Alerts link to the investigation.

- [x] 2.1 Emit `auth.code.locked` (warn) on the locking guess and `auth.code.refused_locked` (info) on a `Locked` claim, in `VerifyEmailUseCase` and `ValidateResetCodeUseCase`, with `flow` and `user_id` (D7). Use-case tests capture the tracing output: 5 wrong guesses then 3 more give exactly 1 locked and 3 refused events, with their fields. Verify with `just test`.
- [x] 2.2 Emit `auth.sign_in.failed` (info) with `user_id` in `LoginUserUseCase` for a wrong password and for a password-less account, and nothing for an unknown email. Tests cover all three and check the error is still `InvalidCredentials`.
- [x] 2.3 Extend the 1.4 no-personal-data test to capture the log events from the same runs. Assert that none contains an email, code, token or password, and that each carries a `trace_id`.
- [x] 2.4 Add the four VictoriaLogs panels to `auth-security.json` and repoint both alerts' `dashboard` annotations (D7). Run `just obs-check`, then `just obs-up` on a fresh stack and confirm the panels show "no data", not errors.
- [x] 2.5 Run `just load auth-abuse`. From each alert's link, confirm the panels list the affected accounts and an entry opens its trace in Tempo. Record the result in design.md Hypothesis.
- [x] 2.6 `just api-types-check` shows no diff and `just ci` is green.

## 3. Slice 3: magic links, bot-check rejections and session anomalies

Delivers these scenarios: Each code flow is counted separately; Bot-check
rejection is counted per form; Reused refresh token is counted; Session theft
signal; and, for the new instruments, Measurements hold no personal data,
Measuring never affects sign-in, No traffic yet and Normal use stays quiet.

- [x] 3.1 Magic links: record `magic_link` outcomes in the `verify_magic_link` handler (`success`; `InvalidToken` is `failure`, `TokenExpired` is `expired`, `TokenLocked` is `locked`; `AlreadyVerified` isn't counted) (D3). Emit `auth.code.refused_locked` with `flow = "magic_link"` and `user_id` in `VerifyMagicLinkUseCase` on `TokenLocked` (D7). Handler test: an account whose email code is locked opens its magic link and gets 1 `magic_link/locked` with no change to `email_code`. Use-case test captures the event.
- [x] 3.2 Bot check: add `auth.bot_check.rejections` (`form`, `reason`) to `AuthMetrics` with closed `BotCheckForm` and `BotCheckReason` enums. `verify_turnstile` takes the form and the metrics, so all four callers name their form (D3). A Turnstile upstream error isn't counted. Handler tests with a rejecting Turnstile stub: a missing and an invalid token on sign-in each count once under `sign_in`, the other forms stay at 0, and responses are unchanged.
- [x] 3.3 Sessions: give `RefreshTokenUseCase` a typed `RefreshError` (`Invalid`, `Expired`, `Reused`, `DeviceMismatch`, `Domain`), mapping each to today's `Authorization` message (D2). Add `auth.session.anomalies` (`kind` = `token_reuse` | `device_mismatch`) and record it in the `refresh` handler. Tests: each variant keeps today's status and body, a reused token still signs out every session and counts once, and a device mismatch counts once.
- [x] 3.4 Add `AuthBotCheckRejectionsHigh` and `AuthSessionAnomalies` to the `auth` vmalert group (D5), with unit tests: 50 rejections stay quiet and 51 fire, 2 anomalies stay quiet and 3 fire, and each resolves. Verify with `just obs-check`.
- [x] 3.5 Add "Bot-check rejections by form" and "Session anomalies" panels with threshold lines to `auth-security.json`, and link both new alerts to them (D4). Run `just obs-check`, and on a fresh `just obs-up` stack confirm they show "no data", not errors.
- [x] 3.6 Extend the no-personal-data test to cover the new instruments' labels (`form`, `reason`, `kind`) and the magic-link event, with the series still fixed.
- [x] 3.7 Add a session-reuse step to `auth-abuse` (replay a rotated refresh token 3 times) and confirm `AuthSessionAnomalies` fires. Confirm `just load auth-mix`, which refreshes sessions, fires no auth alert and counts no anomaly. Bot-check rejections are tested in vmalert only (D6). Record both results in design.md Hypothesis.
- [x] 3.8 `just api-types-check` shows no diff and `just ci` is green.

## 4. Feature done

- [ ] 4.1 Document the auth metrics, the investigation log events, the dashboard and the alert thresholds in `deploy/compose/README.md` (Observability), including the manual Turnstile always-fail check. Fix the `login_user.rs` doc comment and the README so they no longer say Traefik rate limiting exists (it is planned for the production edge, not built).
- [ ] 4.2 Archive the change. Thresholds are revisited after the first month in production (recorded in Hypothesis).
