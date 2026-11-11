# Tasks

The whole feature fits in one slice (appetite: one week). Tasks 1.1 and 1.6 are
early on purpose: they test the two riskiest unknowns (see design.md Hypothesis).

## 1. Slice 1 (first slice): a complete, router-derived API description and generated frontend types, both enforced by the gate

This slice delivers every scenario in `specs/api/spec.md`:
- **The API description covers every served route.** All scenarios.
- **Field presence and nullability are described as sent and accepted.** All scenarios.
- **Fields with a fixed set of values list them.** All scenarios.
- **Error responses are described.** All scenarios.
- **The committed description stays current.** All scenarios.
- **Frontend API types come from the description.** All scenarios.
- **The description is not served in production.** All scenarios.

### Router-derived spec (backend, infrastructure)

- [ ] 1.1 Add `utoipa-axum` (0.3). Add `infrastructure/api/state.rs` with `AppState` and `FromRef` impls (D2). Move `require_fresh_session` to an `Extension` for the user repository. Rewrite `routes.rs` as a value-free `api_routes() -> OpenApiRouter<AppState>` using `routes!`, with `create_router` keeping its signature. Re-path all 26 `#[utoipa::path]` annotations relative to their group (D1). Delete `MOUNTED_ROUTE_TEMPLATES`, `ApiDoc`'s `paths(...)` and `every_openapi_path_is_routable`. Verify: `just test-integration` passes with no test changes, including `security_integration.rs` and `admin_integration.rs`. If neither checks that a demoted admin's old token is rejected, add that test first and keep it green across the refactor.
- [ ] 1.2 Add `pub fn openapi()` returning the split-off spec. Add tests in `openapi.rs` that the spec contains the password-reset, verify-magic and OAuth-unlink operations, and that no path is listed twice. While re-pathing, check each annotation's declared statuses against its handler's return paths (lockout and cooldown included) and fix any that are wrong. Verify: `just test`.
- [ ] 1.3 Add `backend/clippy.toml` `disallowed-methods` (D3). Mark the root health mount and the three middleware test routers with `#[expect(clippy::disallowed_methods, reason = "…")]`. Add `-D clippy::allow_attributes_without_reason` to `just lint` and both CI Clippy steps, and give the 24 existing `allow`s a `reason`. Verify: `just lint` passes. Adding a throwaway plain `.route(` makes it fail, and so does an `expect` without a reason (check locally, don't commit).

### Accurate shapes (backend, application + infrastructure)

- [ ] 1.4 Add `#[serde(default)]` to every `Option` field in request DTOs (D5). Add `RoleName` / `OAuthProviderName` schema-only enums with `#[schema(value_type = …)]` on the role and provider fields, plus a test that they match the domain enums (D6). Verify: `just test`. New `openapi.rs` tests assert that `LoginRequest.turnstile_token` is not required, `UserResponse.pending_email` is not required, `UserResponse.id` is required and not nullable, and `role` lists exactly `user`, `admin` and `super_admin`. An existing integration test that logs in without a Turnstile token still passes.
- [ ] 1.5 Derive `ToSchema` on `ErrorResponse` and add the `ErrorResponses` modifier (D4). Verify: a `just test` unit test asserts that every 4xx/5xx response in `openapi()` has JSON content, and that verify-email's 400 references `VerificationErrorResponse`. Neither schema contains example or description text taken from internal errors.

### Committed spec and generated types

- [ ] 1.6 Add `backend/examples/export_openapi.rs` and the `api-spec`, `api-ts` and `api-types` recipes (D7). Add `openapi-typescript` (exact pin) to `apps/web`, and add `schema.d.ts` to `.prettierignore` and the ESLint ignores. Generate both files and commit them. Then run `pnpm --filter @overkill-todo/web check` with `types.ts` switched to aliases (1.7), *before* fixing anything, and record the error count and kinds in design.md Hypothesis. Verify: `just api-types` runs with Postgres stopped, and running it twice produces no diff.
- [ ] 1.7 Rewrite `types.ts` payload types as aliases of `components['schemas']` under their existing names, and `errors.ts`'s `ApiError` as a derived type (D8). Keep `isAdminRole`, `OAuthProvider` and `HttpClient` unchanged. Resolve each type-check failure by fixing whichever side is wrong, and add any decision change to design.md. Verify: `just test-web` and `just e2e` pass. Searching `apps/web/src` for `interface .*(Request|Response)` finds no hand-written payload types.

### Enforcement

- [ ] 1.8 Add the `api-spec-check`, `api-ts-check` and `api-types-check` recipes. Add `api-types-check` to `just ci`. Add the "API spec current" step to the CI backend job and "API types current" to the web job, and add `backend/openapi.json` to the web job's path filter (D9). Verify: editing a DTO field without regenerating makes `just api-types-check` fail and print `just api-types`. A spec-only change triggers the web job (check `dorny/paths-filter` output on the PR).
- [ ] 1.9 Add the non-blocking pre-commit warning (D9). Verify: staging a DTO change without `backend/openapi.json` prints the warning and still commits. Staging both prints nothing.
- [ ] 1.10 Check that the description stays dev-only. Verify: a `just test-integration` test with dev mode off gets 404 for `/docs` and `/api-doc/openapi.json`. The release-features Clippy step still builds without `swagger-ui`.
- [ ] 1.11 `just ci` green. Merge to `main`, and deploy (image digest change in the deploy config). Run `just smoke <url>` against production: the API behaves exactly as before, and `/docs` returns 404.

## 2. Feature done

- [ ] 2.1 No flag to remove: the change has no runtime switch. Confirm that `backend/openapi.json` and `schema.d.ts` on `main` match a fresh `just api-types`.
- [ ] 2.2 Update docs. CLAUDE.md Backend rules: mount with `routes!`, run `just api-types` after DTO, annotation or route changes, and `allow`/`expect` need `reason`. Also update `backend/ARCHITECTURE.md` (routing and spec section), and the README recipe table (`just api-types`, `just api-types-check`). Verify: `rg MOUNTED_ROUTE_TEMPLATES` returns nothing in the repo, docs included.
- [ ] 2.3 Archive the change (`/opsx:archive`), so `openspec/specs/api/spec.md` is created from the delta.
