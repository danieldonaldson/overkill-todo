# OT-007: Auth anomaly metrics

## Why

As the operator, I want to see and be alerted when sign-in and code checks
start failing at unusual rates, so that I notice credential stuffing, code
guessing and stolen sessions while they happen, not afterwards.

The backend already defends itself: password sign-in needs Turnstile, the
6-digit email and reset codes lock after 5 wrong guesses with an escalating
lock window, and reusing a revoked refresh token signs the account out
everywhere. None of this is counted. A lockout or a revoked-token replay is at
most one warning log line, so an attack looks the same as a quiet day unless
someone happens to search the logs. OT-001 gave us metrics, dashboards and
email alerts. This change points them at authentication.

## What Changes

- The backend counts the outcome of every credential and code check: password
  sign-in, email verification code, password reset code and magic link. Each
  is counted as succeeded, failed, refused because locked, or expired.
- It counts each time a code becomes locked (the guess that reaches the cap),
  separately from guesses refused while it stays locked.
- It counts bot-check (Turnstile) rejections on each protected form, and
  session anomalies: a revoked refresh token presented again, and a refresh
  from a different device than the one the session was issued to.
- None of these measurements carries an email address, user ID, client
  address or code. They are totals per flow and outcome only.
- A new "Auth security" dashboard shows these over time.
- The operator is emailed when failed sign-ins, code lockouts, bot-check
  rejections or session anomalies go above fixed thresholds, and gets a
  resolved notice when they drop back.
- No change to what Visitors and Members see or to any API response.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `observability`: adds measurement of authentication outcomes and abuse
  signals, and alerts on them.

## Appetite

One week.

## Out of Scope

- Per-client-address or per-network (ASN) patterns. Those belong at the edge
  and depend on the production edge, along with rate limiting, which is not
  planned yet.
- Blocking or throttling anything. This change only measures and alerts.
- The audit log of privileged actions (role changes, account deletion, email
  changes). That is a separate change with its own retention.
- OAuth sign-in failures. They are mostly provider errors, which the existing
  error-rate metrics already show.
- Tuning thresholds against real traffic. There is no production yet, so the
  first values are educated guesses, revisited after the first deploy.

## Dependencies

- None blocking. Uses the OT-001 metrics, dashboard and alerting stack.
- No production deploy exists yet. The production checks are recorded for the
  first deploy.

## Ready

- **Riskiest unknown:** whether the alerts can tell an attack from ordinary
  use at this site's tiny traffic. Thresholds that are too low email on a
  forgetful Member, and thresholds that are too high miss a slow campaign. A
  second, smaller unknown: the reset-code flow reports failures only as
  message text today, so a lockout there can't yet be told apart from a wrong
  guess.
- **First slice:** password sign-in outcomes and code lockouts, end to end:
  counted, on the dashboard, alerted. Check it with a local burst of failed
  sign-ins and code guesses that fires the alerts, while the normal-use load
  scenario fires none.

## Impact

`backend/src/infrastructure/telemetry` (new auth instruments), the auth and
verification handlers, typed errors in the reset-code and refresh-token use
cases (same responses as today), `deploy/observability/alerts` (rules and
their unit tests), a new Grafana dashboard, a load-test scenario, and the
observability docs.
