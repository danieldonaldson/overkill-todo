# Tasks

All four slices (see `design.md` Decision 13) are planned here. Slices 3–4
were added after slice 2 shipped, from what slices 1–2 showed.

There is no production host yet (2026-11-07). A slice "ships" when it's merged
with `just ci` green and checked on the full local stack
(`--profile full --profile observability`). The production steps are collected
in group 5 and run at the first production deploy.

## 1. Slice 1 (first slice): backend traces and logs end to end on compose, with redaction

This slice delivers these scenarios:
- **Every request is traceable.** Failed request is traced and marked as an
  error. Client-supplied trace context is ignored. Health checks do not flood
  traces. Successful request produces a full trace, minus the edge span, which
  comes in slice 3.
- **Logs are correlated with traces.** Jump from a log line to its trace. Jump
  from a trace to its logs (backend lines). Search logs by field. Logs outside a
  request.
- **Telemetry never exposes secrets or personal data.** All scenarios.
- **Telemetry is kept for 30 days.** Traces and logs.
- **Observability tools are restricted to the operator.** All scenarios.
- **Telemetry failures never affect Members.** All scenarios.
- **Production requires telemetry to be configured.** All scenarios.

### Backend

- [x] 1.1 Add the OpenTelemetry crates (`opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp` with http-proto/reqwest, `tracing-opentelemetry`, semantic conventions) at mutually compatible versions, plus a Dependabot group for them (the repo uses Dependabot, not Renovate). Verify: `just lint`, `just machete` and `just deny` pass.
- [x] 1.2 Add `TelemetrySettings` (`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_SERVICE_NAME`, `DEPLOYMENT_ENVIRONMENT`, `APP_VERSION`) to `Settings::from_env`. Startup fails when `DEV_MODE=false` and no endpoint is set. Verify: settings unit tests cover the prod-without-endpoint and dev-without-endpoint cases.
- [x] 1.3 Create `infrastructure/telemetry/mod.rs` with `init()`. It returns a flush guard and installs a batch OTLP span exporter with a bounded queue (if no endpoint, a provider with no exporter so trace IDs still exist), the `tracing-opentelemetry` layer and the env filter. Call it from `main.rs` in place of the current subscriber setup, and flush on shutdown. Verify: `just dev` still starts with and without the endpoint set.
- [x] 1.4 Add `infrastructure/telemetry/log_format.rs`: a JSON event formatter that adds `trace_id`, `span_id`, `service.name` and `service.version` at the top level. Keep the pretty output in dev. Verify: a unit test asserts that a log line inside a span carries that span's OpenTelemetry trace ID.
- [x] 1.5 Change `trace_id_middleware` to return the current span's OpenTelemetry trace ID in `X-Trace-Id`. The backend does not extract incoming `traceparent` yet. Verify: `responses_carry_trace_id_header` still passes, and a new test asserts that a supplied `traceparent` header's trace ID is not adopted.
- [x] 1.6 Configure `TraceLayer` span naming (`"{method} {route}"` from `MatchedPath`, `unmatched` fallback), the HTTP semantic-convention attributes, query-value redaction on `url.path`, error status on 5xx, and no spans for `/health` and `/health/ready`. Verify: an integration test with an in-memory span exporter checks the name and attributes for a templated route, an unmatched path and a 5xx, and checks that health produces no span.
- [x] 1.7 Add `#[tracing::instrument(skip_all, name = "repo.<table>.<op>", …)]` to every Postgres repository method. Verify: the in-memory exporter test for login shows at least one `repo.*` child span under the request span.

### Redaction

- [x] 1.8 Add `infrastructure/telemetry/redact.rs` with the name rule from design Decision 8 and its exception list. It's applied at each exit: `RedactingSpanProcessor` for spans, and the log formatters for stdout (a `tracing` layer can't rewrite values other layers see). Verify: unit tests cover every listed name, and the `status_code` exception.
- [x] 1.9 Remove email addresses at the source. Every `#[instrument]` now uses `skip_all` (about 40 sites), and every `email`/`to` field is gone (about 25 sites across handlers, use cases, repositories and the email services). The `google.rs` error log keeps only the OAuth error code, and dev console emails go to stderr, not `tracing`. Verify: `grep -rn 'email = %' backend/src` returns no telemetry fields, and `just test` passes.
- [x] 1.10 Write an end-to-end leak test in `backend/tests/`. It runs register → verify (code) → login → refresh → forgot/reset password → magic link, captures all spans (in-memory exporter) and all formatted log output, and asserts that none of the test email, password, codes, tokens, the CSRF token or cookie values appear. Verify: it passes in `just test-integration`, and fails if the 1.9 change for `register_user` is reverted.

### Pipeline, storage and dashboard

- [x] 1.11 Add `deploy/observability/collector/config.yaml`. It needs an OTLP receiver (traces), `filelog` on Docker container logs with a JSON parser and container → `service.name` mapping, `memory_limiter` and `batch` processors, the redaction/transform processor (header drops, field-name regex, JWT and email masking), and exporters to Tempo and VictoriaLogs. Verify: `otelcol-contrib validate --config` passes in CI (add it to `just lint` or a `just obs-check` recipe that `just ci` calls).
- [x] 1.12 Add `deploy/observability/tempo/tempo.yaml` (single binary, local storage, `block_retention: 720h`). Configure VictoriaLogs with `-retentionPeriod=30d` and a `-retention.maxDiskSpaceUsageBytes` cap. Verify: both containers start healthy with `just obs-up`.
- [x] 1.13 Add Grafana provisioning: Tempo and VictoriaLogs datasources, a derived `trace_id` → Tempo link, Tempo `tracesToLogsV2` → VictoriaLogs, anonymous access and sign-up disabled, admin password from file. Verify: locally, clicking from a log line to its trace and back works for a request made with `curl`.
- [x] 1.14 Add the services to `compose.yaml` under profile `observability`: collector, tempo, victorialogs, grafana. Pin digests, use named volumes and `mem_limit`. Add `just obs-up` and `just obs-down`. Verify: `just obs-up` then `just dev` with `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318` shows traces and logs in local Grafana.
- [x] 1.15 Write a pipeline-outage test. With the stack running, stop the collector and confirm the app still serves sign-in with no visible latency change. Start it again and confirm new traces arrive without restarting the backend. Also start the backend while the collector is down and confirm a warning is logged. Verify: record the results (p95 before and after) in `design.md` Hypothesis.

### Production (compose)

- [x] 1.16 Enable the services in `deploy/compose/compose.prod.yaml`. Grafana is published on `127.0.0.1:3000` only and nothing else is published. Add the SOPS-encrypted Grafana admin password (`deploy/secrets/prod/grafana_admin_password.sops`, decrypted by `just secrets-decrypt prod` into `deploy/compose/.secrets/`), add `OTEL_EXPORTER_OTLP_ENDPOINT`/`DEPLOYMENT_ENVIRONMENT` to the backend env, and pass `APP_VERSION` as a build arg in `backend/Dockerfile` and CI. Verify: `docker compose -f compose.yaml -f deploy/compose/compose.prod.yaml --profile full --profile observability config` renders.
- [x] 1.17 Document the SSH-tunnel access, the deploy order (observability services and endpoint env first, then the backend digest) and the rollback in `deploy/compose/README.md`. Verify: the README section exists and matches design Decision 11 and the Migration Plan.

### Done

- [x] 1.19 `just ci` is green.
- [x] 1.20 Checked on the full local stack (no production host yet): a sign-in's `X-Trace-Id` opens in Grafana, log → trace and trace → log links work (driven with Playwright), the leak search finds nothing, and Grafana and the collector are bound to loopback only. Results are in `design.md` Hypothesis. The production run of the same checks is 5.2.
- [x] 1.22 Update `design.md` with what slice 1 showed (Hypothesis outcome, any revised decisions), then add slice 2 to this file.

## 2. Slice 2: metrics, dashboards and alerts

This slice delivers these scenarios:
- **Request metrics are recorded per route.** All scenarios.
- **Background work and dependencies are observable.** Database health
  (availability, connection use) and telemetry-pipeline health. Job runs and
  leader visibility come in slice 3.
- **Operator is alerted to problems.** Error spike alert. No alert on
  low-traffic noise. Backend down alert. Repeated alerts are grouped. Local
  development never sends alert email. Also the database, storage-volume and
  telemetry-delivery alerts. The stuck-job and leader alerts come in slice 3.
- **Telemetry is kept for 30 days.** Metrics.
- **Observability tools are restricted to the operator.** Covers the new metrics
  and alerting services.

### Backend

- [x] 2.1 Enable the `metrics` features of the OpenTelemetry crates. `telemetry::init` adds a meter provider with a periodic OTLP/HTTP reader (`/v1/metrics`, cumulative temporality) when an endpoint is set, and a provider with no reader otherwise. Shut it down with the guard. Verify: a unit test with the SDK's in-memory metric exporter sees a recorded value, and `just lint`, `just machete` and `just deny` pass.
- [x] 2.2 Record `http.server.request.duration` (seconds, buckets 5 ms … 10 s) with `http.route`, `http.request.method` and `status_class` (`2xx`…`5xx`) in `telemetry::http`, using the same route label as spans (`unmatched` fallback, health probes excluded). Verify: lib tests show a templated route recorded under its template, 1,000 distinct unknown paths producing a single `unmatched` series, and no series for health probes.
- [x] 2.3 Add observable gauges `db.client.connection.count` (`state` = idle/used) and `db.client.connection.max` from the `PgPool`, registered in `main.rs`. Verify: a `#[sqlx::test]` reads both gauges back through the in-memory exporter.

### Pipeline and storage

- [x] 2.4 Add VictoriaMetrics (single node, `-retentionPeriod=30d`, pinned digest, named volume, `mem_limit`) to the `observability` profile, and a metrics pipeline in the collector: OTLP in, then out to VictoriaMetrics' OTLP endpoint. Verify: `just obs-check`, and locally the request histogram is queryable in VictoriaMetrics after a few requests.
- [x] 2.5 Add `postgres_exporter` (pinned digest, the DB credentials from the existing secret in prod) scraped by the collector's `prometheus` receiver; the collector's own telemetry (export failures, queue size) scraped the same way; an `httpcheck` receiver probing the backend's `/health/ready` every 15 s; and a `hostmetrics` filesystem scraper covering the volumes of Tempo, VictoriaLogs and VictoriaMetrics. Verify: each metric family is queryable locally.

### Alerts

- [x] 2.6 Add vmalert and Alertmanager (pinned digests, profile `observability`, no public ports). Rules in `deploy/observability/alerts/*.yml` for: 5xx rate above 2% for 5 m (only above 1 request/minute), p95 above 1 s for 10 m, readiness probe failing for 2 m, database down for 2 m or connections above 80% of max for 10 m, any telemetry volume above 80% for 15 m, and collector export failures for 10 m. Each alert links to its dashboard panel. Verify: `vmalert-tool unittest` covers each rule firing, plus the low-traffic case (1 of 3 requests failing) not firing.
- [x] 2.7 Alertmanager config: `group_by: [alertname]`, `repeat_interval: 4h`, `send_resolved: true`. Locally a `null` receiver; in prod an email receiver whose SMTP password comes from a new SOPS secret (`deploy/secrets/prod/alertmanager_smtp_password.sops`, read with `smtp_auth_password_file`). Verify: `amtool check-config` passes for both configs (in `just obs-check`), and locally a firing test alert shows in the Alertmanager UI with no email sent.

### Dashboard

- [x] 2.8 Provision the VictoriaMetrics datasource (Prometheus type) and a "Service overview" dashboard. It shows request rate, 5xx rate and p50/p95/p99, overall and per route, plus DB connections and probe status, over any range up to 30 days. Latency panels carry a data link to Tempo search (`{ span.http.route = "${route}" }`) for the panel's time range. Verify: Playwright opens the dashboard and follows a route's link to a list of traces for that route.

### Done

- [x] 2.9 On the full local stack: stop the backend and see the backend-down alert fire and then resolve in Alertmanager, and force 5xx responses (DB stopped) and see the error-rate alert fire. Record the results in `design.md`.
- [x] 2.10 `just ci` is green.
- [x] 2.11 Update `design.md` with what slice 2 showed, then add slices 3–4 to this file.

## 3. Slice 3: edge and background jobs

This slice delivers these scenarios:
- **Every request is traceable.** Successful request produces a full trace (now
  including the edge span). Client-supplied trace context is ignored, now
  enforced at the edge, since the backend starts trusting incoming context.
- **Logs are correlated with traces.** The edge's access log line carries the
  trace ID from its own span. Logs outside a request carry the job run's trace
  ID.
- **Background work and dependencies are observable.** Job runs are visible.
  Failed job run. Leader visibility.
- **Operator is alerted to problems.** Stuck background job alert, and the
  leader alert (none or more than one holding the lock for 5 minutes).

### Edge

- [ ] 3.1 Turn on Caddy's `tracing` directive in `Caddyfile` and `Caddyfile.local`, exporting to the collector over OTLP (gRPC 4317 on the compose network). Before it, strip client-supplied `traceparent`/`tracestate` (`request_header -traceparent`, `-tracestate`). Point Caddy's OTLP env at the collector in `compose.yaml` and `compose.prod.yaml`. Remove the collector's `resp_headers.X-Trace-Id` fallback once `traceID` is in the access log. Verify: a request through the edge produces one trace whose root span is the edge's, with the backend request span as its child, and its access log line's `trace_id` matches.

### Backend

- [ ] 3.2 Extract W3C context from incoming headers in `MakeHttpSpan` (`TraceContextPropagator`, already installed globally) and use it as the span's parent. Verify: a lib test asserts that a supplied `traceparent` is adopted by the backend span. Replace `client_supplied_traceparent_is_ignored` with an edge-level check (a `traceparent` sent to the edge does not appear as the trace ID in `X-Trace-Id`), run on the full local stack and recorded in 3.8.
- [ ] 3.3 Run each job iteration (`account_purger`, `database_maintenance`) inside a root span `job.<name>` with an `outcome` attribute (`success`/`failure`), and log failures at error level inside that span. Record `job.run.duration` (histogram, seconds), `job.last_success_timestamp` and `job.interval_seconds` (gauges), labelled by `job`, in `infrastructure/tasks`. Verify: tests with the in-memory span and metric exporters see a `job.account_purger` span with `outcome=success`, the duration and both gauges; and, for a failing run, `outcome=failure` plus an error log line carrying that span's trace ID.
- [ ] 3.4 Add the `leader.is_leader` gauge (0/1) to `tasks::leader`, so every replica reports, labelled by `service.instance.id` (already on the resource). Verify: a `#[sqlx::test]` with two leader loops on one database sees exactly one reporting 1.

### Alerts and dashboard

- [ ] 3.5 Add rules: job not run successfully for `2 × job_interval_seconds + 900` (design Decision 10), and `sum(leader_is_leader) != 1` for 5m (covers none and more than one). Verify: `vmalert-tool` tests cover the hourly purge alerting at 2h15m and not before, no leader, and two leaders.
- [ ] 3.6 Add a "Jobs" dashboard: last run time, outcome and duration per job, a link from a job to its recent `job.<name>` traces, and the current leader instance. Verify: Playwright opens it and follows a job's link to its traces.

### Done

- [ ] 3.7 `just ci` is green.
- [ ] 3.8 On the full local stack: a sign-in's trace starts at the edge, a client `traceparent` is not adopted, the purge job's run appears as a trace and on the Jobs dashboard, and with two backend replicas exactly one shows as leader; stopping the leader moves it to the other. Record the results in `design.md`.
- [ ] 3.9 Update `design.md` with what slice 3 showed.

## 4. Slice 4: Kubernetes

This slice delivers the same scenarios as slices 1–3 on the Kubernetes
manifests. They render but aren't applied anywhere, so "delivered" here means
they render and pass the checks below, not that they are deployed.

- [ ] 4.1 Add `deploy/k8s/observability/` as a kustomize component: the collector as a DaemonSet (OTLP, `filelog` on `/var/log/pods`, `kubeletstats` for volume usage instead of `hostmetrics`), Tempo, VictoriaLogs and VictoriaMetrics as StatefulSets with PVCs, vmalert, Alertmanager, postgres_exporter and Grafana, all reusing the config under `deploy/observability/`. Services are `ClusterIP` only. Verify: `kubectl kustomize deploy/k8s/overlays/example` renders with the component enabled.
- [ ] 4.2 Add NetworkPolicies: OTLP ingress to the collector only from the app namespace, the backend reachable only from the edge (the precondition for trusting incoming trace context, design Decision 3), and nothing in the observability namespace reachable from outside it except through `kubectl port-forward`. Verify: the rendered policies are reviewed against design Decision 11, and a `kubeconform` (or equivalent) check runs in `just ci`.
- [ ] 4.3 Secrets for Grafana admin and the Alertmanager SMTP password come from the same SOPS files as compose. Verify: the rendered manifests reference Secrets, never literal values.
- [ ] 4.4 `just ci` is green, and `design.md` is updated with what slice 4 showed.

## 5. First production deploy (deferred: no production host yet)

- [ ] 5.1 Check host memory and disk headroom against the measured limits (`design.md` Open Questions) before deploying. Add the host's age key under `machines:` in `deploy/secrets/access.yaml` with `env: prod`, then run `just secrets-sync`. Create the Alertmanager SMTP password secret (`printf %s "$PASSWORD" | just secrets-new deploy/secrets/prod/alertmanager_smtp_password.sops`), and set the SMTP host, username, sender and operator address in `deploy/observability/alertmanager/alertmanager.prod.yml`. Verify: `just secrets-decrypt prod` works on the host and writes `alertmanager_smtp_password`.
- [ ] 5.2 Deploy to compose prod: the observability services first, then the backend digest (`deploy/compose/README.md`). Verify with `just smoke <url>`, then repeat the 1.20 checks there: a sign-in's `X-Trace-Id` opens in Grafana through the SSH tunnel, the log ↔ trace links work, a search for the test account's email and password returns nothing, and `https://<domain>` gives 404 for `/grafana` while `:3000`, `:4317`, `:4318` and `:9093` are unreachable. Also fire a test alert (`amtool alert add`) and confirm the operator receives the email and then the resolved notice. Record the results in `design.md` Hypothesis.

## 6. Feature done

- [ ] 6.1 All slices shipped, and telemetry is on in every deployed environment (the endpoint is set, so there's no flag to remove). Verify: the backend in production refuses to start without the endpoint (spec scenario), and the dashboards show live data.
- [ ] 6.2 Update `README.md` (new env vars including `BACKEND_READY_URL`, `just obs-up`/`just obs-down`, how to open Grafana and Alertmanager locally), `backend/.env.example`, `backend/ARCHITECTURE.md` (the `infrastructure/telemetry` module, the `skip_all` instrumentation convention, the redaction rule, metric instruments built from an explicit `Meter`) and `CLAUDE.md` (a one-line rule: never record secrets or emails in spans and logs). Verify: docs reviewed against the final env and recipes.
- [ ] 6.3 Archive the change with `/opsx:archive`. Verify: `openspec/specs/observability/spec.md` exists and `openspec validate --strict` passes.
