# OT-001: Observability

## Why

As the operator running overkill-todo, I want to see how the service behaves in
production and be told when it breaks, so that I find and fix problems before
Members notice them.

Today the backend writes structured JSON logs to stdout and nothing else. There
are no metrics, no traces, no dashboards and no alerts. A slow login, a failing
purge job or a database running out of connections goes unseen until someone
complains. Auth is built and the app is about to grow features, so this is the
cheapest point to add observability: there's little code to instrument and few
habits to change.

## What Changes

- Every backend request produces a trace covering the edge, the handler, the
  use case and its database calls. The response's existing `X-Trace-Id` header
  carries that trace's ID.
- Every backend and edge log line carries the trace ID of the request it belongs
  to. Logs are searchable by any field.
- Request rate, error rate and latency are recorded per route, along with health
  signals for the database, background jobs and the telemetry pipeline itself.
- An operations dashboard, reachable only by the operator, shows the service at
  a glance and links from a log line to its trace, from a trace to its logs, and
  from a slow route to example traces.
- Alerts are emailed to the operator for error spikes, high latency, the backend
  being unreachable, background jobs not running, database trouble, telemetry
  storage filling up, and telemetry not being delivered.
- Secrets and personal data are kept out of telemetry. Passwords, tokens, codes,
  cookies and session data are never recorded, and email addresses are not stored
  in traces or logs.
- All telemetry is self-hosted and kept for 30 days.
- The stack is available locally (opt-in), on the single-host compose deployment
  and in the Kubernetes manifests.
- Production refuses to start without somewhere to send telemetry. Local dev runs
  without it.
- **BREAKING (internal only):** `X-Trace-Id` values become W3C trace IDs. They're
  still 32 hex characters, so nothing that reads the header needs to change.

## Capabilities

### New Capabilities

- `observability`: what the operator can observe about the running service
  (traces, logs, metrics, dashboards, alerts), retention, access to the
  observability tools, and what must never be recorded.

### Modified Capabilities

None. No specs exist yet.

## Impact

- **Backend:** telemetry setup at startup, HTTP and job instrumentation, trace ID
  sourcing for `X-Trace-Id` and logs, removing email addresses from existing span
  fields and log events, and new environment variables.
- **Edge (Caddy):** starts traces and passes trace context to the backend. Its
  access logs carry the trace ID.
- **Deploy:** new services in `compose.yaml` (opt-in profile) and in
  `deploy/compose`, new manifests under `deploy/k8s`, and secrets for the
  dashboard admin and alert email.
- **Hosting cost:** roughly 1 GB more memory on the host, plus disk for 30 days of
  telemetry.

## Appetite

TBD. Suggested: about 3 weeks, in four slices of at most 5 days each.

## Out of Scope

- Frontend telemetry (browser errors, Web Vitals). This is planned as a later
  change.
- Hosted or third-party observability services.
- Alerting from outside the host. If the host itself goes down, nothing on it can
  send an alert. See Risks in `design.md`.
- Business or product analytics.
- On-call rotation, paging apps, SLO and error-budget reporting.

## Dependencies

- An SMTP account that can send alert email. The existing app SMTP account can be
  reused.
- Enough memory and disk on the production host for the extra services.
- No data migrations.

## Ready

- **Riskiest unknown:** whether a trace really joins up from edge to database,
  with log↔trace links working in the dashboard and no secrets or email addresses
  leaking, on the real production host.
- **First slice:** backend traces and logs flowing through the pipeline into
  trace and log storage on the compose deployment, with log↔trace links in the
  dashboard and redaction in place, checked in production with a real login.
