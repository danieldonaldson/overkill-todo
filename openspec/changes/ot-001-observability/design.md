# Design: OT-001 Observability

## Context

See `proposal.md` for why and `specs/observability/spec.md` for the required
behaviour. Here is the current state that shapes the approach:

- **Logging.** `main.rs` sets up `tracing-subscriber` with an `EnvFilter`: JSON
  output in prod, pretty output in dev. `TraceLayer::new_for_http()` wraps the
  router.
- **Trace IDs.** `infrastructure/api/middleware/trace_id.rs` generates a random
  u128 per request, records it as `trace_id` on the current span and returns it
  as `X-Trace-Id`. `backend/tests/security_integration.rs` checks that the header
  is present.
- **Existing spans.** About 15 use cases and some repositories already use
  `#[tracing::instrument]`. Seven of them record a plain `email` field
  (`register_user`, `complete_signup`, `validate_reset_code`,
  `resend_verification`, `verify_email`, `request_password_reset`, plus the field
  that `skip(self)` captures automatically). `auth_handler.rs:125,129` log
  `email`. `code_generator.rs:85` logs `email` at debug level. `google.rs:244`
  logs the Google token-exchange response body.
- **Background work.** Jobs (`account_purger`, `database_maintenance`, hourly)
  run under the advisory-lock leader in `infrastructure/tasks/leader.rs`.
- **Deployment.** Single host via `compose.yaml` plus
  `deploy/compose/compose.prod.yaml`, behind a Caddy edge that already writes
  JSON access logs to stdout. Kubernetes is kustomize in `deploy/k8s` (it renders
  but isn't applied anywhere). Images are pinned by digest, and the backend image
  is `FROM scratch`.
- **Local dev.** `just dev` runs the backend natively, with only `db` in compose.

## Goals / Non-Goals

**Goals:**
- One collector per host is the only thing the backend and edge talk to. Every
  storage choice sits behind it, so swapping one is a config change, not a code
  change.
- Redaction is done twice: in-process (so secrets never leave the backend) and
  in the collector (so a mistake in either layer is caught by the other).
- The application and domain layers stay free of telemetry crates.

**Non-Goals:**
- Tail-based or adaptive sampling. Traffic is low, so 100% sampling is
  affordable for now.
- Log-based alert rules. Every alert is a metric rule.
- High availability for the observability stack itself.

## Hypothesis

We believe the riskiest part, a trace that joins up end to end with working
log↔trace links and no leaks, can be done with stock components and a few
hundred lines of backend code.

Slice 1 tests this on the compose production host, not just locally. The edge
and backend behave differently behind real TLS and a real domain.

We'll know it worked when all of these hold:
- A real sign-in in production returns an `X-Trace-Id` that opens in the
  dashboard.
- The dashboard links from that trace to its log lines and back.
- A search of all stored telemetry for the test account's email, password and
  tokens returns nothing.

If log↔trace linking needs custom glue beyond provisioning config, record that
here and revisit Decision 7.

**Slice 1, local result (2026-11-07, full compose stack + `observability`
profile, real requests through the Caddy edge):**
- A sign-in's `X-Trace-Id` opens in Tempo with nested spans: request →
  `middleware.csrf` → `handler.login` → `usecase.login_user` →
  `repo.user.find_by_email`.
- Log → trace works: an expanded VictoriaLogs line shows **Links → trace_id →
  View trace**.
- Trace → logs works: a span's **Related logs** (Grafana 13's name for "Logs for
  this span") opens the edge and backend lines for that trace. No glue beyond
  provisioning was needed, so Decision 7 stands. Checked by driving Grafana with
  Playwright.
- The edge is correlated already. Caddy logs the backend's `X-Trace-Id` response
  header, and the collector copies it into `trace_id`. Slice 3 still adds real
  edge spans.
- Leak search: VictoriaLogs has no hits for the email, password or magic token
  sent through the edge. The edge's `request.uri` is stored as
  `?token=[REDACTED]`.
- Collector outage: p95 for `GET /api/v1/auth/me` through the edge was 1.0 ms
  with the collector up and 1.4 ms with it stopped (300 requests each, warm).
  Sign-in still answered. After the restart, new traces arrived without
  restarting the backend.
- Found and fixed along the way: Grafana 13 was OOM-killed at 256 MB rendering
  Explore, so its limit is now 512 MB. The collector's own `service.name` from
  the compose label clashed with the backend's, so the backend's identity now
  wins. The backend logs no ongoing warning while exports fail (only the startup
  probe does). The pipeline-health alert in slice 2 covers that.

**Slice 2, local result (2026-11-11, full compose stack + `observability`
profile):**
- Metrics flow end to end: backend OTLP push (every 15s), postgres_exporter,
  the collector's own metrics, vmalert, Alertmanager, VictoriaMetrics, the
  readiness probe and volume usage all land in VictoriaMetrics.
- The "Service overview" dashboard shows rate, 5xx share and p50/p95/p99,
  overall and per route. Playwright clicked a point on the per-route p95 panel,
  followed **Traces for /api/v1/auth/me**, and got a Tempo search for
  `{ span.http.route = "/api/v1/auth/me" }` over the panel's range, listing that
  route's traces.
- **Backend down** (backend container stopped): BackendDown reached
  Alertmanager after 200s, and resolved 90s after the backend came back.
- **5xx spike** (DB stopped, about 2 req/s, half of them sign-ins answering
  500): DatabaseDown after 2m44s, BackendDown after 3m34s (readiness checks the
  DB), and BackendErrorRateHigh ("Backend 5xx rate is 50%") after 6m32s of
  traffic: 5 minutes of `for`, plus the rate window filling and the evaluation
  and grouping delays.
  BackendLatencyHigh fired as well after 11m26s: while the DB was down,
  sign-ins waited out the pool's 10s acquire timeout before answering 500, so
  p95 sat above 1s. Once the DB was back, every alert resolved, with a resolved
  notice, within 6.5 minutes of the last failing request.
- Rule unit tests (`vmalert-tool`, in `just obs-check`) cover every rule firing,
  and the low-traffic case (a steady 33% failing at under 1 request/minute)
  staying quiet. Each test was checked by breaking its rule and watching it
  fail.
- Found and fixed along the way:
  - **BackendDown fired after ~8 minutes, not 2.** A passing probe reports
    `httpcheck_status_ratio{http_status_class="2xx", http_status_code="200"} = 1`;
    a failing one reports a *different* series (no status code) = 0. OTLP has
    no staleness markers, so VictoriaMetrics kept returning the old series' 1
    for about 5 minutes and `max()` stayed at 1. The rule (and dashboard panel)
    now use `max(last_over_time(...[30s]))`, and a rule test reproduces the
    series swap.
  - **Two replicas would overwrite each other's metrics.** Every backend sent an
    identical resource, so their series collided. The resource now carries
    `service.instance.id` (`HOSTNAME`, which is the container ID or pod name,
    else a random UUID per process).
  - **Volume usage reported nothing.** In a container, the host's block device
    isn't in `/dev`, so the `hostmetrics` filesystem scraper counts the volume
    mounts as virtual and skips them. `include_virtual_filesystems: true`
    fixes it. Docker named volumes on one disk all report that disk, so the
    disk alert is in effect a host-disk alert. That's what it should catch.
  - **Memory.** VictoriaMetrics used 233 of 256 MiB (it sizes its caches to
    the limit) and Alertmanager 55 of 64 MiB, so their limits are now 512m and
    128m. See Open Questions.

There is no production host yet (2026-11-07). The same checks run at the first
production deploy (tasks group 5). Until then, the full local stack is where
each slice is checked.

## Decisions

### 1. Components

| Role | Component | Why |
|---|---|---|
| Pipeline | OpenTelemetry Collector (contrib distribution) | Vendor-neutral OTLP intake. It also tails container logs, scrapes Postgres, probes health, redacts, and fans out to storage. |
| Traces | Grafana Tempo 3 (monolithic mode, local disk, no Kafka) | Proven trace store with TraceQL search. Chosen by the operator over VictoriaTraces, which is less mature, and on 3.x at the operator's request. |
| Logs | VictoriaLogs | Low memory use, every JSON field indexed with no label design up front, OTLP and JSON intake. |
| Metrics | VictoriaMetrics (single node) | Prometheus-compatible (PromQL/MetricsQL), small footprint, built-in retention. |
| Alert rules | vmalert | Evaluates rules against VictoriaMetrics and sends to Alertmanager. |
| Alert routing | Alertmanager | Grouping, repeat interval, resolved notices, email. |
| Dashboard | Grafana OSS | Datasources for all three stores, plus correlation links. |

**Alternatives considered:**
- **Loki for logs.** Rejected for this stack because it needs label design up
  front and more memory.
- **Prometheus server.** Rejected: VictoriaMetrics is smaller and the queries
  are the same.
- **Hosted backends.** Rejected by the operator: everything is self-hosted.

All images are pinned by digest, like the app images. Slice 1 pins collector
contrib `0.165.0`, Tempo `3.1.1`, VictoriaLogs `v1.53.0`, Grafana `13.2.5`, and
the VictoriaLogs Grafana plugin `0.33.1` (installed at Grafana start through
`GF_PLUGINS_PREINSTALL_SYNC`, so the first start needs outbound internet).

### 2. Backend telemetry lives in `infrastructure/telemetry`, with no application port

The new module `infrastructure/telemetry/` owns:
- setting up the OTLP exporters for traces and metrics,
- the `tracing` layer stack,
- the redaction layer,
- metric instruments.

`main.rs` calls `telemetry::init(&settings.telemetry, dev_mode)`. It gets back a
guard that flushes on shutdown.

The application and domain layers keep using only the `tracing` macros they
already use. Nothing there calls telemetry directly, so no outgoing port trait
is needed. The CLAUDE.md rule about a real adapter plus a no-op adapter,
refusing the no-op in prod, is met at the composition root:
- If `OTEL_EXPORTER_OTLP_ENDPOINT` is set, the OTLP exporter is the "real
  adapter".
- If it's unset, the backend runs with no exporter (only the local fmt layer),
  which is the "no-op adapter".
- Unset with `DEV_MODE=false` causes `anyhow::bail!`, like SMTP and Turnstile.

Metrics recorded by jobs live in `infrastructure/tasks`, which is already
infrastructure.

**Crates:**
- `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp` using
  `http-proto` + `reqwest` (reuses the existing reqwest/rustls stack and avoids
  pulling in tonic),
- `tracing-opentelemetry`,
- `opentelemetry-semantic-conventions`.

Pick versions that agree with each other when implementing, and run
`just machete` and `just deny` on the result.

**Alternative considered:** a `Telemetry` port in `application/ports/outgoing`.
Rejected because nothing in the application layer would call it. It would be
ceremony.

### 3. Trace ID source and propagation

- **Edge.** Caddy's `tracing` directive starts the root span, exports it to the
  collector over OTLP, and sets `traceparent` on the upstream request. Before
  that, the edge strips any client-supplied `traceparent`/`tracestate`
  (`request_header -traceparent`). This satisfies the "client-supplied context
  is ignored" scenario.
- **Backend.** The backend extracts W3C context from incoming headers, using the
  `TraceContextPropagator`, and uses it as the parent. It trusts that header
  because it's only reachable through the edge: the backend port isn't published
  in prod, and in k8s a NetworkPolicy allows ingress only from the edge.
- **Order.** Until the edge part ships (slice 3), the backend ignores incoming
  context and always starts a root span.
- **`X-Trace-Id`.** `trace_id_middleware` changes to read the trace ID from the
  current span's OpenTelemetry context, instead of generating a random one. When
  telemetry is disabled, a local `SdkTracerProvider` with no exporter still
  assigns real trace IDs, so the header keeps working in dev and in tests.
- **Sampling.** Parent-based, always-on.

### 4. Spans and their names

- **Request spans.** `TraceLayer` is configured with a `make_span_with` that
  names spans `"{method} {route}"`, using `MatchedPath` and falling back to
  `"unmatched"`. It records the HTTP semantic-convention attributes: method,
  route, status code, and `url.path` with query values redacted.
- **No body or headers.** `TraceLayer` records neither, except the allowlist
  `user-agent` and `content-length`.
- **Health endpoints.** `/health` and `/health/ready` are excluded with a filter
  in the span maker.
- **Database calls.** Repository methods get `#[tracing::instrument(skip_all,
  name = "repo.<table>.<op>", fields(db.system = "postgresql"))]`. sqlx 0.9's own
  query events attach to that span as span events, carrying the SQL text but not
  bound parameters.
- **Why not raw query spans.** Per-query spans with no repository context are
  noisier and harder to read. Adding the macro to repository methods is
  mechanical.
- **Jobs.** Each job iteration runs inside a root span `job.<name>` with an
  `outcome` attribute.

### 5. Metrics

Metrics are pushed over OTLP (delta temporality off, cumulative) to the
collector, which remote-writes to VictoriaMetrics.

**Backend instruments:**
- `http.server.request.duration`: histogram in seconds, labelled by `route`,
  `method` and `status_class`. Rate and errors come from the histogram count.
  Buckets: 5ms … 10s.
- `job.run.duration` (histogram) and `job.last_success_timestamp` (gauge),
  labelled by `job`.
- `leader.is_leader`: gauge, 0 or 1, labelled by `instance`.
- `db.client.connection.count`: gauge, labelled by `state` (idle/used), plus
  `db.client.connection.max`. *Renamed in slice 2* from `db.pool.*` to the
  OpenTelemetry database semantic-convention names.

*Decided in slice 2:*
- The HTTP histogram's labels are the semantic-convention names `http.route`
  and `http.request.method`, plus `status_class`, the same route label spans
  use.
- VictoriaMetrics runs with `-opentelemetry.usePrometheusNaming`, so queries
  use Prometheus-style names: `http_server_request_duration_seconds_bucket`,
  `db_client_connection_count`, `http_route`.
- Instruments are built from an explicit `Meter` (production takes the global
  one that `telemetry::init` installs), so tests read them back through the
  SDK's in-memory exporter without touching global state.
- The resource carries `service.instance.id`, so replicas' series stay apart.

**Collected by the collector:**
- `postgres_exporter`, scraped through the collector's `prometheus` receiver.
  Gives availability and `pg_stat_activity` connection counts against
  `max_connections`.
- `httpcheck` receiver probing the backend's `/health/ready` every 15s from
  inside the network. This drives the backend-down alert.
- `hostmetrics` filesystem scraper (on k8s, `kubeletstats`) for storage volume
  usage.
- The collector's own telemetry (`otelcol_exporter_send_failed_*`,
  `otelcol_exporter_queue_size`) for the pipeline-health alert.

**Cardinality:**
- Only route templates become labels; user IDs never do.
- Unmatched paths collapse to `route="unmatched"`.

**Alternative considered:** a Prometheus `/metrics` endpoint scraped by the
collector. Rejected for the backend: OTLP push keeps one exporter path for
traces and metrics, and there's no extra port to keep private. `/metrics`
remains fine for third-party exporters.

### 6. Logs

The backend keeps writing JSON to stdout. A custom JSON formatter replaces the
plain `fmt::layer().json()` and adds these top-level fields:
- `trace_id` and `span_id`, from the OpenTelemetry context of the current span,
- `service.name`,
- `service.version`.

Every compose service uses the json-file log driver with
`labels: com.docker.compose.service` (plus rotation: 10 MB × 5). Docker's log
files carry only the container ID, so this is how each line gets its
`service.name`. The collector keeps only `backend`, `edge`, `web` and `db`, so
it never ingests its own logs or the stack's.

The collector's `filelog` receiver tails:
- in compose, `/var/lib/docker/containers/*/*.log` (read-only bind mount),
- in k8s, `/var/log/pods/...` (DaemonSet).

It parses the JSON, maps container to `service.name`, and exports to
VictoriaLogs over OTLP. Caddy access logs follow the same path. Once the
`tracing` directive is on, Caddy includes `traceID` in its access logs, and the
collector renames that field to `trace_id`.

**Why logs go through stdout instead of an OTLP log exporter:**
- `docker logs` / `kubectl logs` keep working when the stack is down.
- Caddy and Postgres logs use the same path.
- The `opentelemetry-appender-tracing` crate is less mature.

**Alternative considered:** the OTLP log exporter from the backend.
Rejected as the primary path for the reasons above. It can be reconsidered if
stdout parsing proves fragile.

### 7. Correlation in Grafana

All Grafana config is provisioned from files (datasources, dashboards, alert
contact view). Nothing is set up by hand.

- **Log → trace.** The VictoriaLogs datasource gets a derived field on
  `trace_id`, with an internal link to the Tempo datasource.
- **Trace → logs.** The Tempo datasource gets `tracesToLogsV2` with a custom
  query against the VictoriaLogs datasource UID:
  `trace_id:"${__trace.traceId}"`. Its time window is the span ±5 minutes.
- **Slow route → traces.** Dashboard latency panels carry a data link to Tempo
  search: TraceQL `{ span.http.route = "${route}" } | select(duration)` over the
  panel's time range.

VictoriaMetrics exemplar support isn't relied on. The data-link approach works
regardless, so this choice is independent of it.

Slice 1 checks these links on the real stack, as part of the Hypothesis.

### 8. Redaction: in-process and in the collector

**In-process (`infrastructure/telemetry/redact.rs`):**
- Field values whose (lowercased) name contains any of
  `password|passwd|secret|token|code|otp|session|cookie|authorization|csrf`
  become `[REDACTED]`.
- Exceptions: the exact names `status_code`, `http.response.status_code` and
  `otel.status_code`, and the `code.*` prefix (OpenTelemetry source-location
  attributes), because `code` would otherwise match them.
- URL query values whose parameter is `token|code|password|secret|key|state` (or
  any sensitive field name) are replaced before being recorded.
- *Decided in slice 1:* a `tracing` layer can't rewrite the values other layers
  see, so the rule is applied at each exit instead. `RedactingSpanProcessor`
  wraps the span processor and scrubs span and event attributes before any
  exporter sees them. The log formatters (`log_format.rs`) scrub fields as they
  write them, in both JSON and pretty output.

**Source fixes (done in slice 1, wider than first planned):**
- *Every* `#[tracing::instrument]` in the crate now uses `skip_all`, with fields
  listed explicitly. Without it, `skip(self)` silently records every other
  argument via `Debug`. That included emails and passwords in use cases, and
  `CookieJar`/`AuthUser` extractors in handlers, about 40 sites in total.
- Every `email = %…` / `to = %…` field was removed: about 25 sites across
  handlers, use cases, `user_repository` and the SMTP email service.
  `email_domain` is kept where it was useful (MX checks), and the OAuth conflict
  warning logs `user_id`.
- `google.rs` logs status plus the parsed OAuth `error` code, never the body.
- `ConsoleEmailService` (dev only) prints its "emails" to stderr directly rather
  than through `tracing`. It's an inbox stand-in, not telemetry, and redaction
  would otherwise hide the code a developer needs.
- The previous default `TraceLayer` span recorded the full URI, query string
  included (magic-link tokens). It was replaced by the redacting
  `telemetry::http` span.

**Collector (second line):**
- The `redaction` processor masks values of keys matching the sensitive-name
  pattern (with `code` only as a whole key name, e.g. `code`, `reset_code`), plus
  any value matching a JWT or an email address.
- A `transform` processor first flattens parsed log attributes, then replaces
  sensitive URL query values in every attribute and in the body. Caddy's access
  log records the full `request.uri`, so without this a magic-link token from
  the edge would be stored.

**Client IPs:**
- The backend never records client IP in spans or logs.
- The edge's access log keeps `remote_ip`. The operator accepts that it stays
  for 30 days, and it isn't copied into the trace attributes the collector
  forwards.

**Testing:** a unit test in `redact.rs` covers every listed field name. An
integration test runs register → verify → login → refresh → reset against a
captured in-memory exporter plus the fmt output, then asserts that none of the
test secrets or the email address appear (see Requirement: Telemetry never
exposes secrets).

### 9. Retention and disk

- **VictoriaMetrics:** `-retentionPeriod=30d`.
- **VictoriaLogs:** `-retentionPeriod=30d`, plus
  `-retention.maxDiskSpaceUsageBytes` as a hard cap so a log storm can't fill
  the disk.
- **Tempo 3:** `block_retention: 720h`, set in three places:
  `backend_scheduler.provider.compaction.compaction`, `backend_worker.compaction`
  and `overrides.defaults.compaction`. Tempo 3 has no `compactor` block. The
  three paths were confirmed against `/status/config` on 3.1.1.

Each store gets its own named volume, and the disk alert watches each one.

### 10. Alerts

The rules live in `deploy/observability/alerts/*.yml`, loaded by vmalert. The
thresholds are the ones in the spec.

- The error-rate rule requires `sum(rate(requests[5m])) > 1/60`, so very low
  traffic doesn't alert.
- The job-staleness rule is
  `time() - job_last_success_timestamp > 2*interval + 900`. The interval comes
  from a `job_interval_seconds` gauge the job sets, so the rule doesn't hardcode
  1h.
- Leader: `sum(leader_is_leader) != 1` for 5m.
- *Decided in slice 2:* the readiness rule reads
  `max(last_over_time(httpcheck_status_ratio{http_status_class="2xx"}[30s])) < 1`,
  not a plain `max()`. See the slice 2 result under Hypothesis for why.

**Alertmanager settings:**
- `group_by: [alertname]`, `group_wait: 30s`, `repeat_interval: 4h`,
  `send_resolved: true`.
- Email receiver using SMTP credentials from a dedicated secret file. These can
  be the same account as the app's, kept as a separate file so the backend env
  isn't shared with another container.
- *Decided in slice 2:* Alertmanager can't read settings from the environment,
  so `alertmanager.prod.yml` holds the non-secret SMTP settings (host,
  username, sender, operator address). They're `example.com` placeholders until
  the first deploy (task 5.1). Only the password is a SOPS secret
  (`smtp_auth_password_file`).
- Locally, the receiver is a `null` route. Alerts show in the Alertmanager UI
  only.

Each alert email links to the Grafana dashboard panel through an `annotations.
dashboard` URL that uses the operator's tunnel address (see Decision 11).

Vmalert and Alertmanager are configured to report on themselves, so a broken
rule file shows up in the dashboard. Nothing outside the host watches them
(see Risks).

### 11. Access: private, not public

- **Compose prod.** Grafana, Alertmanager, the stores and the collector's OTLP
  ports bind only to the compose network. Grafana is also published on
  `127.0.0.1:3000` on the host, and the operator reaches it with
  `ssh -L 3000:localhost:3000 <host>`. Caddy routes nothing to them, so the
  public domain returns 404.
- **Grafana settings.** Anonymous access off, sign-up off, admin password from a
  SOPS-decrypted secret file.
- **Local.** Ports are published on localhost.
- **k8s.** Services are `ClusterIP` only, reached with `kubectl port-forward`.
  NetworkPolicies allow OTLP ingress to the collector only from the
  `overkill-todo` namespace.

**Alternative considered:** `grafana.$DOMAIN` through Caddy with Grafana login.
Rejected for now. It adds a public login surface for a single operator. Can be
revisited with Cloudflare Access or Tailscale if the tunnel gets annoying.

### 12. Where things live in the repo

```
deploy/observability/          shared config, mounted by compose and k8s
  collector/config.yaml        (+ config.local.yaml overrides)
  tempo/tempo.yaml
  grafana/provisioning/{datasources,dashboards}/
  grafana/dashboards/*.json    service overview, jobs & db, pipeline health
  alerts/*.yml                 vmalert rules
  alertmanager/alertmanager.yml
compose.yaml                   new services under profile `observability`
deploy/compose/compose.prod.yaml  enables them, secrets, loopback Grafana
deploy/k8s/observability/      kustomize component (slice 4)
backend/src/infrastructure/telemetry/{mod,redact,metrics,log_format}.rs
```

New env vars, documented in `README.md` and `backend/.env.example`:
- `OTEL_EXPORTER_OTLP_ENDPOINT` (required when `DEV_MODE=false`),
- `OTEL_SERVICE_NAME` (default `overkill-todo-backend`),
- `DEPLOYMENT_ENVIRONMENT`,
- `APP_VERSION` (baked in at image build from the git SHA).

New just recipes: `just obs-up` and `just obs-down`. They wrap
`docker compose --profile observability`. `just dev` is unchanged.

### 13. Planned slices (revisit after slice 1)

1. **Traces + logs end to end on compose** (in `tasks.md`).
2. **Metrics, dashboards and alerts.** Backend RED metrics, VictoriaMetrics,
   vmalert, Alertmanager email, postgres_exporter, httpcheck, disk and pipeline
   alerts, overview dashboard with route→traces links.
3. **Edge and jobs.** Caddy tracing and stripping of client `traceparent`,
   trusting incoming context in the backend, job spans and metrics, leader gauge,
   jobs dashboard.
4. **Kubernetes.** A kustomize component with the collector DaemonSet, stores as
   StatefulSets with PVCs, Grafana, and NetworkPolicies. It must render with
   `kubectl kustomize overlays/example`.

## Risks / Trade-offs

- **[Risk]** The OpenTelemetry Rust crates release in lockstep and break between
  minor versions → pin compatible versions together, keep all OTel types inside
  `infrastructure/telemetry`, and add a Renovate group so they're upgraded as one.
- **[Risk]** A new `#[instrument]` or log line leaks a secret in a field name the
  regex misses (for example `pw`) → collector-side pattern masking for JWTs and
  emails, the `skip_all` convention, and the end-to-end leak test runs in
  `just test-integration`, so CI catches regressions on the main auth flows.
- **[Risk]** The `code` regex over-redacts useful fields → an explicit exception
  list, covered by a unit test.
- **[Risk]** If the host dies, nothing alerts, because the alerting runs on the
  same host → accepted for this change (Out of Scope). The simplest later fix is
  an external dead-man's switch fed by an always-firing `Watchdog` alert.
  Alertmanager can send to one once it exists.
- **[Risk]** Memory: about 1 GB for collector, Tempo, VictoriaMetrics,
  VictoriaLogs, Grafana, vmalert and Alertmanager → set `mem_limit` on each
  service, check host headroom before slice 1 deploys, and start the
  observability services after the app services so the app keeps priority.
- **[Risk]** Mounting `/var/lib/docker/containers` into the collector exposes
  every container's logs to it → read-only mount, and the collector has no
  public port. This is the standard pattern, and the alternative (a Docker
  logging driver per service) is more config.
- **[Trade-off]** 100% trace sampling costs disk as traffic grows → the disk
  alert at 80% gives warning. Head sampling can then be lowered with one env var
  (`OTEL_TRACES_SAMPLER_ARG`).
- **[Trade-off]** Logs reach VictoriaLogs only via the collector, so a collector
  outage loses log ingestion → `docker logs` still has recent logs, and the
  pipeline-down alert fires.
- **[Risk]** Grafana installs the VictoriaLogs plugin from grafana.com at
  startup (`GF_PLUGINS_PREINSTALL_SYNC`), so a first start without outbound
  internet has no log datasource → the host has outbound internet already
  (image pulls, ACME). If that changes, bake a Grafana image with the plugin.
- **[Risk]** Grafana's Tempo→VictoriaLogs link (`tracesToLogsV2` custom query
  against a non-Loki datasource) doesn't work as expected → tested in slice 1. The
  fallback is a span data link that opens a VictoriaLogs query URL.

## Migration Plan

There's no schema change. Each slice deploys independently:

1. **Ship the stack before the backend needs it.** Deploy the observability
   services in compose prod, then set `OTEL_EXPORTER_OTLP_ENDPOINT` in
   `backend.env` *before* rolling out the backend digest that requires it.
   Otherwise the new backend refuses to start.
2. **Rollback.** Revert the backend digest; the old binary ignores the new env
   vars. The observability services can be stopped at any time without
   affecting the app (see Requirement: Telemetry failures never affect Members).
3. **`X-Trace-Id` stays compatible.** The format is unchanged, so nothing reading
   the header needs to change.

## Open Questions

- Exact memory limits per service. Measured locally after slice 1 (idle with
  light traffic): collector 77 MB, Tempo 187 MB, VictoriaLogs 35 MB, Grafana
  336 MB (OOM at 256 MB). Limits are set at 256/512/256/512 MB. After slice 2
  (light traffic, all services): collector 109 MiB, Tempo 328 MiB, VictoriaLogs
  61 MiB, VictoriaMetrics 233 MiB, vmalert 22 MiB, Alertmanager 55 MiB,
  postgres_exporter 17 MiB, Grafana 233 MiB, about 1.06 GiB in total. Limits:
  collector 256, Tempo 512, VictoriaLogs 256, VictoriaMetrics 512, vmalert 128,
  Alertmanager 128, postgres_exporter 64, Grafana 512 (MiB). Confirm them
  against production host headroom before the first deploy (task 5.1). This
  doesn't change the approach.
- Whether the alert email should use a different sender address from app email.
  This is a config value only.
- "From a slow route to example traces" asks for traces **slowest first**. The
  dashboard link opens a Tempo search for the route over the panel's time range,
  but Tempo returns results newest first and TraceQL can't sort by duration, so
  the operator clicks the Duration column to sort. Either accept that second
  click (and reword the scenario), or narrow the link to traces at or above the
  clicked p95 (`&& duration >= ${__value.raw}s`). That still isn't sorted, but
  it lists only the slow ones. Operator's call.
