# Design

## Context

See proposal.md for why. Current state that shapes the approach:

- OT-001 metrics: instruments live in `infrastructure/telemetry/metrics.rs`
  (`HttpMetrics`), pushed over OTLP to the collector, then VictoriaMetrics
  (Prometheus naming: `auth.attempts` becomes `auth_attempts_total`). Alerts
  are vmalert rules in `deploy/observability/alerts/overkill-todo.yml`, unit
  tested by `just obs-check`. Alertmanager has one route (email), grouped by
  alert name with a 4-hour repeat.
- What each auth path can tell its caller today:
  - Password sign-in: `LoginError::InvalidCredentials` already folds unknown
    email, password-less account and wrong password into one outcome.
  - Email code: `VerifyEmailError` is typed (`CodeInvalid { attempts_remaining }`,
    `CodeLocked`, `CodeExpired`).
  - Magic link: `VerifyMagicLinkError` is typed (`InvalidToken`,
    `TokenExpired`, `TokenLocked`). The link shares the email code's attempt
    row and has no lock of its own.
  - Reset code: `ValidateResetCodeUseCase` returns
    `DomainError::Validation(<message>)` for no code, expired, locked and
    wrong code alike. The outcome is only in the message text.
  - Refresh: `RefreshTokenUseCase` returns `DomainError::Authorization` for
    reuse, expiry and fingerprint mismatch. Reuse and mismatch are only
    `warn!` log lines.
  - Turnstile: one helper, `handlers::verify_turnstile`, used by register,
    sign-in, resend verification and forgot password.

## Goals / Non-Goals

**Goals:**
- Count outcomes where the outcome is already known, without changing any
  response, timing or API description.
- Keep the series count fixed: labels come only from closed enums.

**Non-Goals:**
- Recording client addresses or networks, even hashed.
- Moving outcome detection into the domain. The lock policy stays where it is.

## Hypothesis

We believe absolute thresholds (D5) separate a scripted attack from ordinary
use at this site's traffic. The first slice tests this locally: a k6 abuse
scenario (40 wrong passwords in 10 minutes, 6 locked codes in an hour,
compressed with shorter alert windows in a test profile) must fire the
alerts, and `just load auth-mix` must fire none. vmalert unit tests pin the
exact boundaries. We'll know the real answer only after the first deploy:
record each alert that fires in its first month and whether it was real, then
revise D5.

## Decisions

### D1. Record in the API handlers, from typed use-case results (infrastructure)

A new `AuthMetrics` (next to `HttpMetrics`, built from the global meter, held
in the handler deps) is called by the handlers after each use case returns.
The handler already matches on the result to build the response, so the
outcome is available with no extra work.

- **Rejected: an application-layer `SecurityEvents` port** with an OTel
  adapter and a no-op adapter. It would let use cases record directly, but
  it's three new types and more wiring in `main.rs` for something that is
  pure observability. OT-001 kept all metrics in infrastructure, and this
  stays consistent with that.
- **Rejected: deriving outcomes from HTTP status and route.** A wrong code,
  a locked code and an expired code are all 400 with different bodies, so
  this would mean parsing response bodies in middleware.

### D2. Make the two untyped use cases return typed errors (application)

- `ValidateResetCodeUseCase` gets a `ResetCodeError` like `VerifyEmailError`:
  `NoCode`, `Expired`, `Locked`, `Invalid { attempts_remaining }`,
  `Domain(DomainError)`. Its `From<ResetCodeError> for DomainError` produces
  exactly today's messages, so the response bodies don't change. Unknown email
  and missing code stay one variant, `NoCode`, so the endpoint still doesn't
  reveal whether an account exists.
- `RefreshTokenUseCase` gets a `RefreshError`: `Invalid`, `Expired`,
  `Reused`, `DeviceMismatch`, `Domain(DomainError)`. Each maps to today's
  `Authorization` message.
- `attempts_remaining` needs `register_attempt`'s claimed count, which the
  reset flow ignores today (`Claimed(_)`). Keep it, as `verify_email` does.

The response stays the same, and a test per variant asserts the status and
body against today's.

### D3. Instruments and labels

| OTel name | Kind | Labels (closed sets) |
|---|---|---|
| `auth.attempts` | counter | `flow` = `password_sign_in` \| `email_code` \| `reset_code` \| `magic_link`; `outcome` = `success` \| `failure` \| `locked` \| `expired` |
| `auth.code.lockouts` | counter | `flow` = `email_code` \| `reset_code` |
| `auth.bot_check.rejections` | counter | `form` = `register` \| `sign_in` \| `resend_verification` \| `forgot_password`; `reason` = `missing` \| `invalid` |
| `auth.session.anomalies` | counter | `kind` = `token_reuse` \| `device_mismatch` |

The labels are Rust enums with `as_str()`, never caller-supplied strings,
so the scrubbing spec can't be broken by mistake and the series count is
fixed (4×4 + 2 + 4×2 + 2 = 28).

- **Lockout = the failed guess that leaves `attempts_remaining == 0`.** That
  guess is counted as `failure` and also increments `auth.code.lockouts`.
  Later guesses that `register_attempt` refuses are `outcome=locked`. This
  matches "a lockout is counted once".
- Unknown email, password-less account and wrong password are all
  `password_sign_in/failure` (spec: counts don't reveal accounts).
- `AlreadyVerified`, `NotFound` (email code) and `NoCode` (reset) are not
  guesses and are not counted. Expired is counted as `expired`.
- A Turnstile upstream error (Cloudflare unreachable) is a 500 and not a
  rejection. The 5xx alert already covers it.
- `verify_turnstile` takes a `BotCheckForm` argument and the metrics, so
  every caller has to name its form.

### D4. Dashboard

New provisioned dashboard `deploy/observability/grafana/dashboards/auth-security.json`
(uid `auth-security`): sign-ins by outcome, sign-in failure ratio, code
checks by flow and outcome, lockouts, bot-check rejections by form, session
anomalies. Each alert's threshold is drawn as a line on its panel. Alert
`dashboard` annotations link to the matching panel, as OT-001 does.

### D5. Alert rules (new `auth` group, `severity: warning`)

| Alert | Expression (sketch) | `for` |
|---|---|---|
| `AuthSignInFailuresHigh` | `sum(increase(auth_attempts_total{flow="password_sign_in",outcome="failure"}[15m])) > 30` | 0m |
| `AuthCodeLockoutsHigh` | `sum(increase(auth_code_lockouts_total[1h])) > 5` | 0m |
| `AuthBotCheckRejectionsHigh` | `sum(increase(auth_bot_check_rejections_total[15m])) > 50` | 0m |
| `AuthSessionAnomalies` | `sum(increase(auth_session_anomalies_total[1h])) >= 3` | 0m |

- `for: 0m`, because the window is already in the range. A `for` on top of
  that would delay a burst by another window.
- `increase()` on sparse counters extrapolates. Unit tests pin values just
  either side of each threshold. If extrapolation makes the boundary fuzzy,
  switch to `sum(rate(...[w])) * w_seconds` with `round()`, decided in the
  slice.
- A counter that appears for the first time (first failure since start)
  starts at 1, and `increase()` misses that first increment. Accepted: these
  thresholds are about bursts, not single events.
- `severity: warning` vs the existing `page`: Alertmanager routes everything
  to email today, so the label changes nothing yet. It lets a later change
  route security alerts differently.
- Thresholds are absolute, not ratios, because traffic is tiny. A failure
  *ratio* would fire on 2 of 3 sign-ins at night. They are a first guess,
  revisited after the first deploy (Hypothesis).

### D6. Testing the alerts end to end locally

- New k6 scenario `loadtest/scenarios/auth-abuse.js`: wrong-password
  sign-ins against seeded accounts, and wrong codes until locked.
- Bot-check rejections are tested in the vmalert unit tests only. Turnstile
  is a no-op locally. A manual check with Cloudflare's always-fail test
  secret is noted in the docs, not automated.
- To fire alerts in minutes, not an hour, the hypothesis check reads the
  dashboard counts and the vmalert unit tests prove the rules. Running the
  real 1-hour window locally isn't required.

## Risks / Trade-offs

- [Thresholds are guesses with no production traffic] → Hypothesis records
  each firing after the first deploy. Thresholds live in one rules file with
  unit tests, so changing them is cheap.
- [A distributed slow campaign stays under every threshold] → Accepted for
  now. Per-address detection belongs at the edge, later. The dashboard
  still shows the trend.
- [Typed errors drift from today's messages, changing the API] → Per-variant
  response tests, and `just api-types-check` in CI shows no description diff.
- [Metrics alter auth timing] → A counter increment is nanoseconds next to an
  Argon2 verify. Every failure path already does the same Argon2 work, and
  recording happens after it.
- [Benign token reuse (a retried refresh on a flaky network) looks like
  theft] → The threshold is 3 per hour, not 1. The web client already shares
  one rotation across tabs.

## Migration Plan

No schema change. Deploy order: rules and dashboard (config, needs
`dc restart vmalert grafana`), then the backend digest. Rules over metrics
that don't exist yet evaluate to empty and don't fire. Rollback: the previous
backend digest stops emitting, and the alerts go quiet.
