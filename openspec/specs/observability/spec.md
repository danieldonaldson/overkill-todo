# observability Specification

## Purpose
Lets the operator see how the running service behaves (requests, errors,
latency, background work, database health) through traces, logs, metrics,
dashboards and alerts, without telemetry ever exposing secrets or Members'
personal data.

## Requirements

### Requirement: Every request is traceable
The system SHALL record a trace for every HTTP request handled by the backend.
The trace SHALL start at the edge and include the backend's handling of the
request and each database call made for it. Every backend response SHALL carry
an `X-Trace-Id` header containing that trace's 32-character lowercase hex ID.

#### Scenario: Successful request produces a full trace
- **WHEN** a Member signs in successfully
- **THEN** the response carries an `X-Trace-Id` header of 32 lowercase hex characters
- **AND** within 1 minute the operator can open that trace in the operations dashboard
- **AND** the trace shows the edge, the backend request and at least one database call as nested steps with their durations

#### Scenario: Failed request is traced and marked as an error
- **WHEN** a request fails with a 5xx response
- **THEN** its trace is recorded and marked as an error
- **AND** the trace shows the route template (for example `/api/v1/auth/oauth/{provider}`), not the raw path

#### Scenario: Client-supplied trace context is ignored
- **WHEN** a Visitor sends a request with their own `traceparent` header
- **THEN** the system starts a new trace and does not adopt the supplied trace ID

#### Scenario: Health checks do not flood traces
- **WHEN** the liveness and readiness endpoints are polled
- **THEN** no traces are recorded for those requests

### Requirement: Logs are correlated with traces
Every log line written while handling a request SHALL carry the trace ID of that
request, including the edge's access log line for it. The operator SHALL be able
to search logs by any recorded field.

#### Scenario: Jump from a log line to its trace
- **WHEN** the operator finds a backend error log line in the operations dashboard
- **THEN** the line shows a trace ID that opens the matching trace in one click

#### Scenario: Jump from a trace to its logs
- **WHEN** the operator is viewing a trace
- **THEN** one click shows all log lines (edge and backend) carrying that trace ID

#### Scenario: Search logs by field
- **WHEN** the operator searches logs for a given `user_id` over the last 24 hours
- **THEN** every backend log line with that `user_id` in that window is returned

#### Scenario: Logs outside a request
- **WHEN** a background job or startup code writes a log line
- **THEN** the line is stored and searchable, and carries the job's own trace ID if it runs inside a traced job run

### Requirement: Request metrics are recorded per route
The system SHALL record, per route template and response status class, the
request rate, error rate and latency distribution of the backend.

#### Scenario: Dashboard shows rate, errors and latency per route
- **WHEN** the operator opens the service overview dashboard
- **THEN** it shows request rate, 5xx error rate and p50/p95/p99 latency, overall and per route, for a selectable time range up to 30 days

#### Scenario: From a slow route to example traces
- **WHEN** the operator sees a latency spike on a route
- **THEN** one click lists example traces for that route in that time window, slowest first

#### Scenario: Unknown paths do not create new series
- **WHEN** a Visitor requests thousands of distinct non-existent paths
- **THEN** they are all counted under a single "unmatched" route, not one series per path

### Requirement: Background work and dependencies are observable
The system SHALL record each run of every background job (outcome and duration),
which replica currently holds the background-work leader lock, database health
(availability and connection use), and the health of the telemetry pipeline
itself.

#### Scenario: Job runs are visible
- **WHEN** the account purge job runs
- **THEN** the operator can see its last run time, outcome and duration in the dashboard, and its run appears as a trace

#### Scenario: Failed job run
- **WHEN** a background job run fails
- **THEN** the failure is recorded as a failed run with an error log line carrying that run's trace ID

#### Scenario: Leader visibility
- **WHEN** two backend replicas are running
- **THEN** the dashboard shows exactly one of them as the background-work leader

### Requirement: Operator is alerted to problems
The system SHALL email the operator when any of these conditions persists for
the stated duration, and SHALL send a resolved notice when it clears:

- backend 5xx responses above 2% of requests for 5 minutes (only counted when there is at least 1 request per minute)
- backend p95 latency above 1 second for 10 minutes
- the backend not responding to its readiness check for 2 minutes
- no replica holding the background-work leader lock for 5 minutes, or more than one holding it
- a background job with no successful run for more than twice its interval plus 15 minutes
- the database unavailable for 2 minutes, or connections above 80% of its maximum for 10 minutes
- any telemetry storage volume above 80% full for 15 minutes
- telemetry failing to reach storage for 10 minutes

#### Scenario: Error spike alert
- **WHEN** more than 2% of backend requests return 5xx for 5 minutes
- **THEN** the operator receives one email naming the alert, the current value and a link to the relevant dashboard
- **AND** receives a resolved email once the rate stays below the threshold

#### Scenario: No alert on low traffic noise
- **WHEN** 1 of 3 requests in a quiet 5-minute window returns 5xx
- **THEN** no error-rate alert is sent

#### Scenario: Backend down alert
- **WHEN** the backend stops answering its readiness check for 2 minutes
- **THEN** the operator receives a backend-down alert

#### Scenario: Stuck background job alert
- **WHEN** the hourly account purge has not succeeded for 2 hours 15 minutes
- **THEN** the operator receives a job-not-running alert naming the job

#### Scenario: Repeated alerts are grouped
- **WHEN** an alert condition stays true for several hours
- **THEN** the operator receives at most one reminder email every 4 hours, not one per evaluation

#### Scenario: Local development never sends alert email
- **WHEN** the observability stack runs locally and an alert condition is met
- **THEN** the alert is visible in the local alert view and no email is sent

### Requirement: Telemetry never exposes secrets or personal data
Telemetry (traces, logs and metrics) SHALL NOT contain passwords, password
hashes, access or refresh tokens, verification or reset codes, magic-link
tokens, OAuth codes or state, CSRF tokens, cookies, `Authorization` headers,
session identifiers, or request and response bodies of auth routes. Email
addresses SHALL NOT appear in traces or backend logs; a Member is identified by
user ID. Client IP addresses SHALL appear only in edge access logs.

#### Scenario: Sign-in leaves no secrets behind
- **WHEN** a Member signs in with email and password and refreshes their session
- **THEN** a search of all telemetry from that window finds neither the password, the email address, the access token, the refresh token nor the CSRF token

#### Scenario: Registration and reset flows leave no secrets behind
- **WHEN** a Visitor registers, verifies their email with a code, and later resets their password with a reset code
- **THEN** no telemetry contains the email address, either code, the magic-link token or the new password

#### Scenario: Sensitive query parameters are stripped
- **WHEN** a request URL carries a query parameter named `token`, `code`, `password`, `secret`, `key` or `state`
- **THEN** the recorded URL in traces and logs shows the parameter's value replaced with `[REDACTED]`

#### Scenario: A sensitive field added by mistake is still scrubbed
- **WHEN** a backend log line or trace attribute is written with a field whose name contains `password`, `token`, `secret`, `code`, `otp`, `session` or `cookie`
- **THEN** the stored value is `[REDACTED]`

#### Scenario: Upstream error bodies are not stored
- **WHEN** a call to an external identity provider fails
- **THEN** the log line records the status and error kind but not the provider's response body

### Requirement: Telemetry is kept for 30 days
The system SHALL keep traces, logs and metrics for 30 days and delete older data
automatically.

#### Scenario: Data within retention is queryable
- **WHEN** the operator queries for a trace, log line or metric from 29 days ago
- **THEN** it is returned

#### Scenario: Old data is removed
- **WHEN** telemetry is older than 30 days plus one compaction cycle (at most 24 hours)
- **THEN** it is no longer stored

### Requirement: Observability tools are restricted to the operator
The operations dashboard, alert views and every telemetry ingestion or query
endpoint SHALL NOT be reachable from the public internet. The dashboard SHALL
require the operator to sign in. App roles (Member, Admin, Super Admin) grant no
access to it.

#### Scenario: Visitor cannot reach telemetry
- **WHEN** a Visitor requests the dashboard, a metrics endpoint or a telemetry ingestion endpoint on the public domain
- **THEN** the request does not reach any of them (the edge returns 404, or the port is closed)

#### Scenario: Admin of the app has no dashboard access
- **WHEN** an Admin is signed in to the app
- **THEN** that session gives no access to the operations dashboard

#### Scenario: Operator signs in to the dashboard
- **WHEN** the operator connects through the private access path and signs in with the dashboard credentials
- **THEN** the dashboards, log search, trace search and alert views are available

### Requirement: Telemetry failures never affect Members
The backend SHALL keep serving requests normally when the telemetry pipeline or
any telemetry storage is slow or unavailable. Telemetry produced during the
outage MAY be lost.

#### Scenario: Pipeline down
- **WHEN** the telemetry pipeline is stopped
- **THEN** Members can still sign in and use the app with no added latency beyond 10 ms at p95
- **AND** after the pipeline returns, new telemetry flows again without restarting the backend

#### Scenario: Backend starts while the pipeline is down
- **WHEN** the backend starts in production while the telemetry pipeline is unreachable
- **THEN** the backend starts and serves requests, and logs a warning that telemetry is not being delivered

### Requirement: Production requires telemetry to be configured
The backend SHALL refuse to start outside development mode when no telemetry
destination is configured. In development mode it SHALL start without one and
log that telemetry is disabled.

#### Scenario: Production without telemetry destination
- **WHEN** the backend starts with development mode off and no telemetry destination set
- **THEN** it exits at startup with an error naming the missing setting

#### Scenario: Local development without the stack
- **WHEN** a developer runs the app locally without the observability stack
- **THEN** the backend starts normally, logs that telemetry is disabled, and `X-Trace-Id` headers are still returned
