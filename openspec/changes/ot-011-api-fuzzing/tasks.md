# Tasks

This change is dev tooling plus small backend fixes, with nothing to deploy. A
slice ships when it's on `main` with `just ci` green. Slice 2 (the CI job, the
nightly workflow and reconsidering `positive_data_acceptance`) was added after
slice 1 shipped and `design.md` Hypothesis recorded what the first triage showed.

## 1. Slice 1 (first slice): accurate security description, and a local fuzz run that's clean and part of `just ci`

This slice delivers these scenarios from `specs/api/spec.md`:
- **Authentication is described as clients really use it**: all scenarios.
- **The backend's behaviour is fuzzed against its description**: all scenarios.
- **Fuzzing runs in the gate**: full gate run locally, short run is repeatable.

### Security description (design Decision 4)

- [x] 1.1 Add the `session_cookie`, `csrf_header` and `refresh_cookie` security schemes in `openapi::finalize`. Verify: `just api-spec` output has all three under `components.securitySchemes`.
- [x] 1.2 Make `document()` / the `Guarded` methods in `routes.rs` add `session_cookie` wherever `require_auth()` applies and `csrf_header` wherever `require_csrf()` adds its 403. Set `refresh_cookie` on `/auth/refresh`. Remove all hand-written `security(("bearer_auth" = []))` attributes. Verify: `grep -rn bearer_auth backend/src` is empty, and `openapi.json` shows the expected requirements on `/auth/me`, `/users/password/change` (both schemes), `/auth/login` (none) and `/health` (none).
- [x] 1.2a Stop accepting Bearer tokens (Decision 4a): `extract_token` reads only the session cookie, `authorization` leaves the CORS allowed headers, and the integration tests send the session cookie instead. Add an integration test that a valid JWT sent only as `Authorization: Bearer` gets 401 `token_missing` on `/auth/me`. Note the change in `docs/security/threat-model.md` if it lists accepted credentials. Verify: `grep -rn "Bearer" backend/src backend/tests` finds only that test, and `just test` and `just test-integration` pass.
- [x] 1.3 Add `openapi.rs` tests: every security requirement names a defined scheme; every operation with a guard-documented 401 requires `session_cookie`; every operation with the CSRF 403 requires `csrf_header`; public routes require nothing. Verify: `just test` passes, and putting back one `bearer_auth` attribute makes the first test fail and name the route.
- [x] 1.4 Run `just api-types` and commit `backend/openapi.json` and `apps/web/src/lib/api/schema.d.ts`. Verify: `just api-types-check` and `just test-web` pass with no frontend edits.

### Fuzz tooling (Decisions 1–3)

- [x] 1.5 Pin `uv` in `mise.toml`. Add `apifuzz/pyproject.toml` (Schemathesis pinned to an exact version, `exclude-newer` set to the 24-hour rule) and `apifuzz/uv.lock`. Exclude `apifuzz/` from prettier/eslint where needed. Verify: `mise install && uv run --project apifuzz --frozen schemathesis --version` prints the pinned version, and `just lint-js` passes.
- [x] 1.6 Add `apifuzz/run.sh`:
  - create and migrate `overkill_todo_fuzz_<pid>`;
  - start the backend binary with `DEV_MODE=true` on `FUZZ_API_PORT`;
  - wait for `/api/v1/health/ready`, failing after a timeout;
  - run the fuzzer against the committed `openapi.json`;
  - stop the backend and drop the database from a `trap`.

  Verify: a run leaves no `overkill_todo_fuzz_*` database behind (`\l`), even when interrupted with Ctrl-C. With the database down, the script exits non-zero before any fuzzing. Accounts in the dev database are unchanged afterwards.
- [x] 1.7 Add `just fuzz-api seed="<fixed>" examples="25"`, which calls `run.sh`. Verify: two runs on the same commit report the same request counts and the same result.

### Auth and checks (Decisions 5–7)

- [x] 1.8 Add the Member and Admin auth providers in `apifuzz/hooks.py`. They register `test-fuzz-<run>-<n>@example.test` through register → verify-email (code `123456`), promote the Admin with a parameterised `UPDATE` on the fuzz database, and attach the cookies and `X-CSRF-Token`. Add the after-call hook that marks credentials stale on a 401 and signs in a new Member next time, with a cap on re-registrations per run. Verify: a run that fuzzes `DELETE /users/me` and `/auth/logout` still gets 2xx responses on `/auth/me` afterwards, and the run log shows the re-registrations.
- [x] 1.9 Set up the passes: Visitor (no auth, all operations), signed in (Member for Member routes, Admin for `/admin/*`) and Member on `/admin/*` (expects 403). Verify: on a scratch branch, removing `require_auth()` from one group makes the Visitor pass fail on it, and removing the admin guard makes the Member-on-admin pass fail.
- [x] 1.10 Turn on the built-in checks from Decision 6 (not `positive_data_acceptance`). Add the custom `error_body` check with its deny list, plus a unit test for the deny list with allowed and denied sample messages. Verify: `uv run --project apifuzz pytest` passes. On a scratch branch, a handler returning `AppError::Validation(format!("{sqlx_err}"))` fails the run and shows the message, and a route using bare axum `Json` fails with a 415/422 finding.
- [x] 1.11 List the excluded operations with a reason in the Schemathesis config (the OAuth callback). Verify: the run summary shows every other described operation as tested.

### First run and triage (Hypothesis)

- [x] 1.12 Run `just fuzz-api` and sort every distinct failure into *fix the description*, *fix the backend* or *tune the fuzzer*. Record the counts and the outcome in `design.md` Hypothesis. Verify: the Hypothesis section has the numbers, and any decision it changes is updated in Decisions.
- [x] 1.13 Fix the description and backend findings. Add `format`, `minLength`, `maxLength` and `pattern` constraints to DTOs where the validator enforces them. Fix any handler that returns a 5xx or a non-standard error. Add a unit or `#[sqlx::test]` test for each backend defect. Split anything larger than about a day into its own task or change, and leave it out of the run with a reason that references it. Verify: `just test`, `just test-integration` and `just api-types-check` pass, and `just fuzz-api` exits 0.
- [x] 1.14 Add `fuzz-api` to the `ci` recipe and record the short run's duration. Verify: `just ci` is green, and the fuzz step adds less than about 3 minutes.

## 2. Slice 2: fuzzing in CI and nightly, and the stricter check reconsidered

This slice delivers these scenarios from `specs/api/spec.md`:
- **Fuzzing runs in the gate**: change to the backend, pull request that only
  changes the frontend, nightly finding.

Slice 1 showed (design Hypothesis outcome) that the noise came from the harness,
not from validation rules, so `positive_data_acceptance` is tried again here.

- [x] 2.1 Make `run.sh` CI-ready: keep each pass's output in `apifuzz/out/<pass>.log`, and when `GITHUB_STEP_SUMMARY` is set, append the seed, the result of each pass, its failures and the `just fuzz-api seed=… examples=…` command that reproduces them. Verify: a local run with `GITHUB_STEP_SUMMARY=/tmp/summary.md` writes a summary that names a failure planted on a scratch change, and a clean run writes a clean one.
- [ ] 2.2 Add the `api-fuzz` job to `ci.yml`: gated on `changes.outputs.backend` (add `apifuzz/**` to that filter), Postgres service, rust-cache, sqlx-cli, uv from `.mise.toml` through mise, `just`-free (`./apifuzz/run.sh`), the hooks unit tests first, logs uploaded on failure. Add it to the `images` job's `needs`. Verify: `actionlint` passes, and the job is green on the push to `main`.
- [ ] 2.3 Add `.github/workflows/fuzz-nightly.yml` (cron plus `workflow_dispatch` with optional `seed` and `examples` inputs; defaults: a random seed and 500 examples). It prints the seed and the reproducing command in the job summary and uploads the logs. It shares no concurrency group with CI and gates nothing. Verify: `actionlint` passes, and a manual dispatch on `main` completes and shows its seed in the summary.
- [x] 2.4 Reconsider `positive_data_acceptance`: run with it on, sort every distinct failure as in 1.12, and either fix the description or backend so it can stay on, or leave it off with the reason in the config and design. Record the outcome in design.md Hypothesis. Verify: `just fuzz-api` and `just ci` are green, and the config states the decision.

## 3. Feature done

- [ ] 3.1 Document `just fuzz-api` (what it checks, how to reproduce a failure with a seed, how to add an exclusion) in `README.md`. Add the "dev-mode differences a client can see" note to `backend/ARCHITECTURE.md`. Mention in CLAUDE.md that a new route is fuzzed automatically and must stay clean. Verify: the docs list the same command, defaults and exclusions as `apifuzz/`.
- [ ] 3.2 Archive the change (`/opsx:archive`), syncing the `api` spec delta. Verify: `openspec validate --specs` passes, and `openspec/specs/api/spec.md` contains the three new requirements.
