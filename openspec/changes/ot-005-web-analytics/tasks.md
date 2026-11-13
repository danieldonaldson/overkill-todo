# Tasks

## 1. Slice 1 (first slice): anonymous page views through our own origin

Delivers from `specs/analytics/spec.md`:
- Page views are recorded anonymously (all scenarios)
- Distinct Visitors are counted without identifying them (all scenarios)
- Browsers that opt out are not measured (all scenarios, for page views)
- The site works the same whether analytics is on, off or failing (all scenarios except "Analytics blocked by the browser" with Google sign-in, which needs events)
- Analytics traffic stays on the site's own address (all scenarios)
- Only the operator can see analytics reports (all scenarios)

- [x] 1.1 Add the `analytics` compose profile to `compose.yaml` (D1, D2): `analytics-db` (`postgres:18`, own volume, healthcheck, `mem_limit`) and `umami` (image pinned by digest, `DATABASE_URL` to `analytics-db`, `APP_SECRET`, `CLIENT_IP_HEADER=x-analytics-client-ip`, `DISABLE_TELEMETRY=1`, `DISABLE_UPDATES=1`, port `127.0.0.1:3001:3000`). Add `just analytics-up` / `analytics-down` next to `obs-up`. Verify: `just analytics-up` brings both up healthy and `http://localhost:3001` shows the Umami login. Record in design.md D8 whether the pinned version has built-in retention.
- [x] 1.2 Edge routing (D3, D4) in `deploy/compose/Caddyfile.local` and `deploy/compose/Caddyfile`: a global `servers` block with `trusted_proxies` (private ranges locally; a dated Cloudflare-ranges snippet in prod) and `client_ip_headers CF-Connecting-IP X-Forwarded-For`; a `handle /stats/*` block ahead of the catch-all that removes any incoming `X-Analytics-Client-IP`, sets it to `{client_ip}`, strips `/stats` and proxies only `/script.js` and `/api/send` to `umami:3000`, and answers 404 for anything else. Verify with `caddy validate` on both files, and against the `full` + `analytics` stack: `curl /stats/script.js` → 200 JS, `curl /stats/` and `/stats/login` → 404, `/api/v1/health` still reaches the backend.
- [x] 1.3 Runtime config: add `UMAMI_WEBSITE_ID` to `src/lib/config.ts` (placeholder-aware like `TURNSTILE_SITE_KEY`), `public/config.js`, and `docker-entrypoint.d/40-render-public-config.sh` (`PUBLIC_UMAMI_WEBSITE_ID`, empty by default), plus `PUBLIC_UMAMI_WEBSITE_ID: ''` on `web` in `compose.yaml`. Verify with a Vitest test for the config value (set, unset, placeholder) and by checking the rendered `/config.js` in the `full` stack.
- [x] 1.4 Add `public/scripts/analytics.js` and load it from `Base.astro` with `<script is:inline src="/scripts/analytics.js" defer>` (D5): does nothing if the ID is unset/placeholder or if DNT/GPC is set; otherwise injects the Umami tracker with `data-host-url` = `location.origin + '/stats'`, `data-exclude-search`, `data-exclude-hash` and `data-do-not-track`. Verify `tests/csp.spec.ts` still passes with no new hash (`just e2e`).
- [x] 1.5 Playwright flows in `apps/web/tests/analytics.spec.ts` (mocked API, `/stats/*` routed by the test): with no website ID, no request goes to `/stats/` on any page; with an ID, opening `/register` sends one page view to `/stats/api/send` whose URL is the path only; opening `/verify-magic?token=abc#x` sends a payload containing neither `abc` nor `#x`; Home → Settings sends two page views (view-transition move); with `/stats/*` aborted or delayed 10 s, registration still completes with no visible error; with GPC or DNT emulated, no `/stats/` request is made; no analytics cookie or storage key is set; no CSP violation is logged on any page (`just e2e`).
- [x] 1.6 Hypothesis check (design Hypothesis): an edge-agnostic script under `scripts/` (takes the base URL, so it can be re-run against the Traefik edge) that, against the `full` + `analytics` stack, creates a test site through Umami's API, posts collect requests through the edge with two different `X-Forwarded-For` addresses and one repeated, and asserts 2 sessions. Also checks with `psql` that no IP address is stored in the analytics database. Run it and record the result in design.md's Hypothesis.
- [x] 1.7 Production deploy config (D2, D7): add `analytics-db`, `umami` (loopback `127.0.0.1:3001` only) to `deploy/compose/compose.prod.yaml` with `analytics_db_password` and `umami_app_secret` as compose secrets; set `PUBLIC_UMAMI_WEBSITE_ID` on `web` from the deploy env (empty means off). Add both secrets to `deploy/secrets/access.yaml`, run `just secrets-sync`, and create them with `just secrets-new`. Verify `just secrets-check` and `docker compose -f compose.yaml -f deploy/compose/compose.prod.yaml config` pass, and that `analytics-db` is not in the `backup` service.
  - _Done:_ no `access.yaml` entry was needed: without one a secret follows its level (prod: `daniel`), like `grafana_admin_password`. `config` needs `--no-env-resolution` without decrypted secrets on the machine. Umami can't read files, so the prod `umami` wraps its entrypoint to build `DATABASE_URL` / `APP_SECRET` from the secrets; checked by running it with throwaway secrets.
- [x] 1.8 Docs: in `deploy/compose/README.md`, write down the edge contract from D3/D4 (the two `/stats` routes, 404 otherwise, client address trusted only from known proxies and passed in Umami's configured header) with the Caddy config and the Traefik mapping, so the production edge can implement it. In it and `README.md`, also cover starting analytics locally, operator access (`ssh -L 3001:localhost:3001 <host>`), changing the default `admin` password on first start, creating the site and setting `PUBLIC_UMAMI_WEBSITE_ID`, rollback (unset the ID), and that analytics data is not backed up. Verify by following the local steps from a clean `just analytics-up` to a page view visible in Umami.
- [x] 1.9 `just ci` green. There is no production deploy yet, so record the first-deploy steps here instead: implement the D3/D4 edge contract in Traefik and re-run the 1.6 check against it, deploy the analytics services, change the admin password, create the site, set the ID, check with `just smoke <url>`, confirm `/stats/` returns 404 publicly, and after a day compare Umami's visitor count with Cloudflare Web Analytics (within about 20%).

## 2. Slice 2: sign-up and sign-in steps as named events

Delivers from `specs/analytics/spec.md`:
- Sign-up and sign-in steps are recorded as named events (all scenarios)
- Browsers that opt out are not measured (the remaining scenarios, for events)
- The site works the same whether analytics is on, off or failing ("Analytics blocked by the browser" with Google sign-in)

- [ ] 2.1 `src/lib/analytics.ts` (D5): `track(event, props?)` typed as a discriminated union of exactly the spec's table (event names and each property's allowed values), so a free-text value fails `astro check`; calls `window.umami?.track` in `try/catch`, never awaits or throws. Plus `reasonFor(error)` (D6), mapping `ApiErrorException` status / `code` / `error_code` and network errors to the fixed `reason` values. Verify with Vitest: every mapping row, `track` is a no-op when `window.umami` is missing or throws, and a `// @ts-expect-error` test that an unknown event or free-text property doesn't type-check.
- [ ] 2.2 Call `track()` after the API call settles (D6) in `RegisterForm` (`signup-submitted` / `signup-rejected`), `VerifyEmailForm` and `VerifyMagicForm` (`email-verified`, `via` `code` / `link`), `LoginForm` (`signin-succeeded` / `signin-rejected`), `OAuthButtons` (`oauth-started`, before navigation), `ForgotPasswordForm`, `ResetPasswordForm` and `SettingsPanel` (deletion). No call may await analytics or change what the form shows. Verify `just test-web`.
- [ ] 2.3 Extend `tests/analytics.spec.ts`: the sign-up funnel scenario (page view, `signup-submitted`, `email-verified` `via` `code`, in order); a server-rejected email gives `signup-rejected` `invalid` whose payload has neither the address nor the server's message; the login form with the API unreachable gives `signin-rejected` `server` and the usual error; "Continue with Google" sends `oauth-started` before leaving (if the test shows it lost, delay navigation by at most 150 ms, design Risks); with `/stats/*` aborted, Google sign-in still navigates; with GPC or DNT, no event is sent. Also assert every sent event name and property value is in the spec's table (`just e2e`).
- [ ] 2.4 Check in Umami (local `full` + `analytics` stack, normal browser) that a funnel built from `/register` → `signup-submitted` → `email-verified` shows one completed sign-up. Record the steps in the README's analytics section. `just ci` green. Note "check the funnel in production" for the first deploy (deploy/compose/README.md).

## 3. Slice 3: 90-day retention and its alert

Delivers from `specs/analytics/spec.md`:
- Analytics data is kept for 90 days (all scenarios)

- [ ] 3.1 `analytics-retention` compose service (profile `analytics`, D8), built like `backup`: once a day, in one transaction, delete Umami 3.5.0's event-data and event rows older than 90 days, then sessions with no remaining events (column and table names from the pinned schema, in a `.sql` file under `deploy/analytics/`). After a successful run, push `analytics_retention_last_success_timestamp_seconds` to VictoriaMetrics' import endpoint; a failed run pushes nothing. Production reads the DB password from `analytics_db_password`. Verify against a seeded database: rows at 89 and 91 days, only the 91-day rows (and their orphaned sessions) go.
- [ ] 3.2 vmalert rule in `deploy/observability/alerts/overkill-todo.yml`, same shape as the existing job-not-running rule, firing when `time() -` the metric exceeds 2 d 15 m, with the job named in the alert; a case in `alerts/tests/overkill-todo.test.yml` for firing and not firing. Verify `just obs-check`.
- [ ] 3.3 Docs (both READMEs): retention period, what the job deletes, how to run it by hand, and the alert. `just ci` green. Note "confirm a retention run and the metric in production" for the first deploy.

## 4. Slice 4: Kubernetes manifests

Delivers the same behaviour on the optional k8s target (`deploy/k8s/`). No new spec scenarios.

- [ ] 4.1 Manifests for `analytics-db` (StatefulSet + PVC, not in any backup), `umami` (Deployment, digest-pinned, same env as compose, secrets from a Secret, ClusterIP only) and the retention job (CronJob), in their own kustomize component so analytics can be left out.
- [ ] 4.2 The `/stats` rules from the edge contract in `deploy/k8s/base/ingress.yaml` (two paths to Umami with the prefix stripped, 404 for the rest, `Cookie`/`Authorization` dropped, client address passed only from trusted proxies). Note in `deploy/k8s/README.md` how this depends on the ingress controller.
- [ ] 4.3 Verify with `kubectl kustomize` (and `kubeconform` if CI already uses it) and in a local cluster if one is set up; otherwise record the check as a first-deploy step for k8s.

## 5. Feature done

- [x] 5.1 After slice 1 ships (or, with no production, after 1.6), record what it showed in design.md's Hypothesis, revise decisions, and add the remaining slices here: named events in the auth forms with the typed `track()` (D5, D6); the retention job and its alert (D8); k8s manifests for the analytics services and the `/stats` ingress route.
- [ ] 5.2 Switch the feature on (set `PUBLIC_UMAMI_WEBSITE_ID` in production). The flag stays as the off-switch, so no removal. Note it as a first-deploy step while there is no production.
- [ ] 5.3 Update README/docs for any env vars or setup changed by later slices.
- [ ] 5.4 Archive the change (`/opsx:archive`), creating `openspec/specs/analytics/spec.md`.
