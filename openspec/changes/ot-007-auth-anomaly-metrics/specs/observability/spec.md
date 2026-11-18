## ADDED Requirements

### Requirement: Authentication outcomes are measured
The system SHALL count every password sign-in, email verification code check,
password reset code check and magic-link check, by flow and outcome. The
outcomes are succeeded, failed, refused because the code is locked, and
expired. It SHALL count separately each time a code becomes locked, each
bot-check rejection per protected form, and each session anomaly (a revoked
refresh token presented again, or a refresh from a different device than the
session was issued to). These counts SHALL carry no email address, user ID,
client address, code or token. The operator SHALL see them on an "Auth
security" dashboard for a selectable time range up to 30 days.

#### Scenario: Failed sign-in is counted
- **WHEN** a Visitor tries to sign in with a wrong password
- **THEN** within 1 minute the dashboard's failed password sign-ins rise by one
- **AND** the Visitor gets the same response as before this change

#### Scenario: Unknown email counts the same as a wrong password
- **WHEN** a Visitor tries to sign in with an email that has no account
- **THEN** it is counted as a failed password sign-in, the same as a wrong password, so the measurements do not reveal which emails have accounts

#### Scenario: Successful sign-in is counted
- **WHEN** a Member signs in with the correct password
- **THEN** the dashboard's successful password sign-ins rise by one

#### Scenario: A lockout is counted once
- **WHEN** a Visitor enters a wrong email verification code 5 times, then tries 3 more times while the code is locked
- **THEN** the dashboard shows 5 failed code checks, 1 code locked, and 3 checks refused because locked

#### Scenario: Each code flow is counted separately
- **WHEN** a password reset code is guessed wrongly until locked, and a magic link is opened for an account whose email code is locked
- **THEN** the dashboard shows one lockout for the reset-code flow and one magic-link check refused because locked, and neither is counted under email verification

#### Scenario: Expired code is not counted as a guess
- **WHEN** a Member enters a reset code after it has expired
- **THEN** it is counted as expired, not as a failed or locked check

#### Scenario: Bot-check rejection is counted per form
- **WHEN** a sign-in request arrives with a missing or invalid bot-check token
- **THEN** the dashboard's bot-check rejections for the sign-in form rise by one, and other forms' counts do not change

#### Scenario: Reused refresh token is counted
- **WHEN** a refresh token that has already been used is presented again
- **THEN** the dashboard's session anomalies (token reuse) rise by one
- **AND** all of that Member's sessions are signed out, as before this change

#### Scenario: Measurements hold no personal data
- **WHEN** the operator lists every label and value on the authentication measurements after a run of sign-ins and code checks by several Members
- **THEN** none contains an email address, user ID, client address, code or token
- **AND** the number of distinct series does not grow with the number of Members or attempts

#### Scenario: Measuring never affects sign-in
- **WHEN** the telemetry pipeline is down while Members sign in and check codes
- **THEN** sign-in and code checks behave and respond exactly as when it is up

#### Scenario: No traffic yet
- **WHEN** the operator opens the dashboard on a newly started system with no sign-ins
- **THEN** each panel shows zero or "no data", not an error

### Requirement: Operator is alerted to authentication abuse
The system SHALL email the operator, in the same way and with the same
grouping and reminders as its other alerts, when any of these conditions
holds, and SHALL send a resolved notice when it clears:

- more than 30 failed password sign-ins within 15 minutes
- more than 5 codes becoming locked within 1 hour, across all code flows
- more than 50 bot-check rejections within 15 minutes, across all forms
- 3 or more session anomalies within 1 hour

#### Scenario: Credential stuffing burst
- **WHEN** 40 sign-ins with wrong passwords arrive within 10 minutes
- **THEN** the operator receives one email naming the failed sign-ins alert, the current count and a link to the Auth security dashboard
- **AND** receives a resolved email once the 15-minute count stays at 30 or below

#### Scenario: A forgetful Member does not alert
- **WHEN** one Member gets their password wrong 6 times and then resets it
- **THEN** no alert is sent

#### Scenario: Code guessing across many accounts
- **WHEN** codes for 6 different accounts become locked within 1 hour
- **THEN** the operator receives a code lockouts alert

#### Scenario: Session theft signal
- **WHEN** 3 revoked refresh tokens are presented again within 1 hour
- **THEN** the operator receives a session anomalies alert

#### Scenario: Normal use stays quiet
- **WHEN** the system runs its normal-use load scenario
- **THEN** none of the authentication alerts fires

#### Scenario: Local development never sends email
- **WHEN** an authentication alert condition is met in the local observability stack
- **THEN** the alert is visible in the local alert view and no email is sent

### Requirement: Authentication abuse can be investigated
The system SHALL write a log event, linked to its request trace, each time a
code becomes locked, each time a code check is refused because the code is
locked, and each time a password sign-in fails for an email that belongs to
an account. Each event SHALL name its flow and the account's user ID, and
SHALL carry no email address, code, token, password or client address. A
sign-in for an email with no account SHALL write no event naming an account.
The operator SHALL see recent lockouts, and lockouts and failed sign-ins per
account, on the Auth security dashboard, and the authentication alerts SHALL
link there. Events are kept for the log retention period (30 days).

#### Scenario: Lockout names the account
- **WHEN** a Visitor guesses an email code wrongly until it locks
- **THEN** within 1 minute the dashboard's recent lockouts show one entry with the email-code flow, the account's user ID and a link to the request trace

#### Scenario: Targeted guessing is visible
- **WHEN** one account's code is guessed until locked and then tried 10 more times
- **THEN** the dashboard shows one lockout and 10 refused checks for that account

#### Scenario: Stuffing versus brute force
- **WHEN** 40 wrong-password sign-ins arrive, spread over 20 accounts
- **THEN** failed sign-ins per account shows 20 accounts with 2 each

#### Scenario: Unknown email names no account
- **WHEN** a sign-in fails for an email with no account
- **THEN** it is counted as before and no log event names an account

#### Scenario: Investigation logs hold no secrets
- **WHEN** the operator searches the logs after failed sign-ins and code guesses by several Members
- **THEN** no authentication event contains an email address, code, token, password or client address

#### Scenario: Alerts link to the investigation
- **WHEN** the code lockouts alert or the failed sign-ins alert fires
- **THEN** its email links to that alert's per-account panel
