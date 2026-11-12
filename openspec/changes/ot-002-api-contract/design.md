# Design

## Context

See proposal.md (Why) for motivation and specs/api/spec.md for the required
behaviour. This section covers only the current state that shapes the approach.

- `infrastructure/api/openapi.rs` declares `ApiDoc` with a hand-maintained
  `paths(...)` list of 18 handlers. 26 routes are served. Most of the missing
  handlers already carry `#[utoipa::path]` annotations; they just aren't
  registered.
- `routes.rs` keeps a third hand-maintained list, `MOUNTED_ROUTE_TEMPLATES`, which
  holds paths but no methods. The only test, `every_openapi_path_is_routable`,
  checks that the spec's paths are a subset of that list. A route added to the
  router is checked by nothing.
- Annotations use absolute paths (`/api/v1/auth/register`), while the router
  nests groups under `/api/v1`, `/auth`, `/users` and `/admin`. Each group has its
  own state type (`Arc<AuthHandlerDeps>`, …) and its own `route_layer`s. Admin
  routes use `from_fn_with_state(users, require_fresh_session)`.
- utoipa-gen 6.0.1 (`component.rs:56`) marks a field required unless it has
  `skip_serializing_if`, `double_option` or `default`. It doesn't matter whether
  the field is an `Option`. So the four `turnstile_token: Option<String>` request
  fields are currently described as required.
- `AppError` serializes as `ErrorResponse { code, message }`
  (`infrastructure/api/error.rs`), which has no `ToSchema`. Verification routes
  return `VerificationErrorResponse` (application DTO, already `ToSchema`).
- `UserResponse.role`, `AdminUserResponse.role` and `SetUserRoleRequest.role` are
  `String`. The values come from domain `UserRole`. `oauth_providers` is
  `Vec<String>`, with values from domain `OAuthProvider` (google, apple).
- Frontend `apps/web/src/lib/types.ts` (173 lines) and `lib/api/errors.ts` declare
  every payload shape by hand. Some names differ from the backend's (`User` vs
  `UserResponse`, `AdminUserRow` vs `AdminUserResponse`, `RegisterRequest` vs
  `InitiateRegistrationRequest`).

## Goals / Non-Goals

**Goals:**

- The spec cannot drift from the router: both come from the same declarations.
- Exporting the spec needs no database, no environment and no adapters.
- Frontend code keeps its current type names, so the switch is a small change.

**Non-Goals:**

- No change to any route's behaviour, status codes, payloads, middleware order
  or protection.
- No generated HTTP client (see proposal Out of Scope).
- No runtime validation of responses against the spec.

## Hypothesis

We believe the router can be moved to `utoipa-axum` with a single application
state, without changing how any route is protected. And we believe the existing
integration suite (`just test-integration`, which drives the real router)
will catch any change in protection. Task 1.1 tests this before any other work.
If the suite passes unchanged and `security_integration.rs` / `admin_integration.rs`
are green, the refactor preserved behaviour.

For drift: we believe most mismatches are optional-vs-required and `string` vs
literal unions, not wrong field names. Task 1.6 generates types from the
first complete spec and counts `astro check` errors before any frontend fix. If
there are more than about 40, or any field name is wrong, record that here and
re-check the appetite.

**What slice 1 showed** (recorded in task 1.6, before any fix):

- **Router refactor:** confirmed. All 666 existing tests passed unchanged, plus a
  new end-to-end demoted-admin test written before the refactor. The only snag
  was a schema named only in `params(...)` (`OAuthMode`), which `routes!`
  doesn't collect. The `every_schema_ref_resolves` test caught it.
- **Drift:** 14 `astro check` errors. None was a wrong field name.
  - 10 were test fixtures building a `User` without `created_at` and `role`,
    which the backend always sends. The frontend was wrong: the hand-written
    type had made both optional.
  - 3 were test mocks returning partial `VerifyEmailResponse` /
    `ResendVerificationResponse` bodies. `user`, `success` and `message` are
    always sent. The frontend was wrong.
  - 1 (`auth.ts`, `expires_at`) exposed a spec bug. utoipa marks every `Option`
    nullable, even when `skip_serializing_if` means it's omitted rather than
    sent as `null`. Fixed in the backend with `#[schema(nullable = false)]` on
    the 13 such response fields (D5 addendum).
- Well under the 40-error threshold, so the appetite holds.

**Rebase onto main** (before 1.8): main had moved 30 commits on, so the router
and handler changes since then were brought into the new structure.

- `POST /verify-magic` (confirm a magic link) joined the GET redirect in one
  `routes!`. Its 400 is `VerificationOrStandardError`, like verify-email.
- The profile group gained `require_fresh_session` on main. It's now a
  `.require_fresh_session()` guard, so its 401 is documented. The guard's text
  now names all three causes: password reset, account purge, role change.
- The OAuth callback kept main's always-redirect behaviour, with `ApiPath` /
  `ApiQuery` on the outer handler.
- 4 new `astro check` errors, all from test fixtures added on main. 3 built a
  `User` without `created_at`/`role`, and 1 resolved `verifyEmail` without
  `user`. The frontend was wrong in every case. The `DashboardNav` "cached
  account without a role" case stays, as an explicit cast.
- New code from main that the 1.3 lints caught: 2 telemetry test routers got
  `expect(disallowed_methods)`, and 2 `allow`s got a `reason`.

## Decisions

### D1. Derive the spec from the router with `utoipa-axum` (infrastructure)

Mount every documented route with `OpenApiRouter` and `routes!(handler)`. The
method and path then come from the handler's `#[utoipa::path]`, and nesting
adds the prefixes. Delete `MOUNTED_ROUTE_TEMPLATES`, the `paths(...)` list in
`ApiDoc` and `every_openapi_path_is_routable`: a described route that isn't
served becomes impossible by construction. `ApiDoc` keeps `info`, `tags` and the
error schemas, and is passed in with `OpenApiRouter::with_openapi`.

Annotation paths become relative to their group: `path = "/register"` under
`/auth` under `/api/v1`. All 25 annotations change. `utoipa-axum` 0.3.0 requires
`utoipa ^6` and `axum ^0.8.4`, which matches our versions.

_Alternatives:_ (a) Keep the lists and add the reverse test. The third list
still drifts from the real router, and methods stay unchecked. (b) Write an
integration test that sends a request to every documented `(method, path)` and
expects something other than 404/405. That only catches one direction, and needs
the DB. (c) Write a custom macro that registers both. That reinvents `utoipa-axum`.

### D2. One value-free route tree over `AppState` (infrastructure)

The spec must be exportable without real dependencies, but route groups bind
state with `.with_state(value)` today. The fix:

- Add `AppState { auth, verification, oauth, profile, admin, db_pool }`
  (`infrastructure/api/state.rs`), with `FromRef` impls for each `Arc<…Deps>`
  and `PgPool`. Handlers keep their `State<Arc<XDeps>>` signatures, because
  axum extracts them through `FromRef`.
- Add `fn api_routes() -> OpenApiRouter<AppState>`, which declares every group,
  nest and `route_layer` and takes no values.
- `create_router(deps, origin)` keeps its signature, so `main.rs` and
  `tests/common` don't change. It calls `api_routes().split_for_parts()`, applies
  `.with_state(AppState::from(deps))`, mounts Swagger UI from the split-off spec
  (dev only, as today), and applies the outer layers.
- Add `pub fn openapi() -> utoipa::openapi::OpenApi`, which returns
  `api_routes().split_for_parts().1` plus the error-response modifier (D4).
- `require_fresh_session` takes the user repository from an `Extension` instead of
  `from_fn_with_state`. It's provided at the outermost layer next to the existing
  `Extension(crypto)`. This follows the pattern already used for crypto.

_Alternatives:_ build the real router with lazy-pool dummy deps in the exporter.
That would tie the export to every adapter's constructor (`tests/common` needs
about 250 lines to do this). Or keep per-group spec functions and duplicate the
nest prefixes in a separate spec builder, which brings back drift on prefixes.

### D3. Undocumented routes need an explicit, reasoned exemption (infrastructure)

Add `backend/clippy.toml` with `disallowed-methods` for `axum::routing::Router::route`,
`axum::routing::Router::route_service` and `utoipa_axum::router::OpenApiRouter::route`
(its pass-through that skips the spec). Clippy already runs with `-D warnings`, so a
plain `.route(` fails `just lint`. An exempt route needs
`#[expect(clippy::disallowed_methods, reason = "…")]`. The reason is mandatory
crate-wide: `just lint` and both CI Clippy steps add
`-D clippy::allow_attributes_without_reason`. The 24 existing
`#[allow(...)]`s, none of which has a `reason`, become `reason = "…"` (moving
their comment where they have one). This turns CLAUDE.md's existing "no `allow`
without a comment" rule into a check.

Starting exemptions:

- **Root `/health` and `/health/ready`.** These are the loopback container
  HEALTHCHECK, and duplicate `/api/v1/health*`. Describing both would produce
  duplicate operation IDs.
- **The three middleware unit-test routers** in `csrf.rs`, `verification.rs` and
  `security_headers.rs`. These are test-only and never served.

The OAuth callback's test-bypass query parameter is not a route, and stays
undocumented (dev only).

_Alternative:_ a test that diffs the router against the spec. axum has no route
introspection, so this would have to go back to a hand-kept list.

### D4. Error bodies: `ErrorResponse: ToSchema` plus a filling modifier (infrastructure)

Derive `ToSchema` on `ErrorResponse`. Add an `ErrorResponses` `utoipa::Modify`
in `openapi.rs` that, for every operation:

1. adds a 500 response if the operation doesn't declare one, and
2. gives every declared 4xx/5xx response that has no body the `ErrorResponse`
   body.

Routes that return the verification body keep declaring
`body = VerificationErrorResponse` explicitly. A unit test asserts that every
4xx/5xx response in `openapi()` has JSON content. Correct status codes stay the
job of each annotation. While re-pathing in 1.2, each annotation's statuses are
checked against its handler's return paths, including lockout and cooldown.

_Alternative:_ write `body = ErrorResponse` in every annotation. That's more
verbose, and easy to forget on the next route.

### D5. Field presence follows serde exactly (application DTOs)

- **Request `Option` fields** get `#[serde(default)]`. This makes them
  non-required in the spec, and serde already treats them that way (a missing
  `Option` deserializes to `None`), so accepted input is unchanged.
- **Response `Option` fields:** those with `skip_serializing_if` are already
  optional. Those without it are always sent, as `null` when empty, so they
  stay required and nullable. Both are accurate, and need no change.
- _Addendum (slice 1):_ response `Option` fields with `skip_serializing_if` also
  get `#[schema(nullable = false)]`. They are omitted, never sent as `null`, and
  utoipa would otherwise describe them as nullable.
- A unit test in `openapi.rs` pins the important cases: `LoginRequest.turnstile_token`
  is not required, `UserResponse.pending_email` is not required, and
  `UserResponse.id` is required and not nullable.

### D6. Fixed-value strings get schema-only enums (application DTOs)

Add `RoleName` and `OAuthProviderName` (`application/dto/schema_enums.rs`) as
`#[derive(ToSchema)] #[serde(rename_all = "snake_case")]` enums. Reference them
with `#[schema(value_type = RoleName)]` and
`#[schema(value_type = Vec<OAuthProviderName>)]` on the existing `String` fields.
Serialization and validation stay exactly as they are, including
`SetUserRoleRequest`'s current 400 for an unknown role. A unit test asserts that
each enum's values equal the domain enum's `to_db_str()` / `as_str()` values,
so adding a domain variant without updating the schema fails `just test`.

_Alternatives:_ derive `ToSchema` on the domain enums. That puts an API-doc
framework in domain, against the layering rule. Or change the DTO field types to
the enums. That changes the error for a bad role from a validation 400 to a
deserialization error: a behaviour change.

### D7. Export and file locations

- **Exporter:** `backend/examples/export_openapi.rs` prints
  `overkill_todo::infrastructure::api::openapi().to_pretty_json()` and a trailing
  newline to stdout. It's an _example_, not a `[[bin]]`, so the Dockerfile's
  `cargo build --locked` doesn't compile or link it. The spec's maps are
  `BTreeMap`-backed, so output order is stable.
- **Spec:** `backend/openapi.json`. The backend owns it, and the backend CI job
  can check it with no Node.
- **Types:** `apps/web/src/lib/api/schema.d.ts`, generated by `openapi-typescript`
  (web devDependency, exact version pinned). It's added to `.prettierignore` and
  the ESLint ignores, so formatting tools never rewrite it.
- **Recipes:** `just api-spec` / `just api-spec-check` (Rust only),
  `just api-ts` / `just api-ts-check` (Node only), and `just api-types` /
  `just api-types-check` (both). `just ci` gains `api-types-check`. Each check
  regenerates the file and runs `git diff --exit-code`, then prints the
  regenerate command on failure. This mirrors `db-prepare` / `db-prepare-check`.

### D8. Frontend keeps its names; payload types become aliases (web)

`types.ts` re-exports `components['schemas'][…]` under the existing names
(`User = components['schemas']['UserResponse']`, `UserRole = RoleName`, and so on).
`isAdminRole` stays. `OAuthProvider = 'google'` stays as a frontend-only type,
because it means "providers this UI offers", not the API's set. `errors.ts`'s
`ApiError` becomes `Partial<ErrorResponse> & Partial<VerificationErrorResponse>`,
and its narrowing helpers are unchanged. `HttpClient` doesn't change.

When `astro check` fails, decide each mismatch: if the backend is right, fix the
frontend. If the frontend reflects real behaviour, fix the DTO or annotation. Add
any mismatch that changes a decision above to this document.

### D9. Enforcement

- **CI backend job:** step "API spec current" (`just api-spec-check`) after
  Clippy.
- **CI web job:** step "API types current" (`just api-ts-check`) before
  `check`. Add `backend/openapi.json` to the web job's path filter, so a
  spec-only change runs the web checks.
- **Pre-commit:** add a non-blocking warning when staged files touch
  `backend/src/application/dto/`, `backend/src/infrastructure/api/` or
  `backend/src/infrastructure/api/handlers/` and `backend/openapi.json` isn't
  staged. It doesn't run cargo, because CI is the gate.
- **CLAUDE.md (Backend rules):** "Mount routes with `routes!` in `api_routes()`.
  After changing a DTO, handler annotation or route: `just api-types` and commit
  `backend/openapi.json` and `apps/web/src/lib/api/schema.d.ts`."

### D10. Middleware statuses are documented by the guard that applies them (infrastructure)

_Added during slice 1._ Statuses that come from middleware (401 from
`auth_middleware` and `require_fresh_session`, 403 from `require_verified_email`
and `csrf_middleware`) apply to whole groups. Writing them on each annotation would
drift. A `Guarded` extension trait on `OpenApiRouter<AppState>` in `routes.rs`
(`require_auth()`, `require_verified_email()`, `require_fresh_session()`,
`require_csrf()`) applies the `route_layer` _and_ adds the status to every
operation it covers, unless the handler's annotation already declares it. The
CSRF guard reuses `csrf::is_excluded_path`, so it only claims 403 on the
mutating, non-excluded operations. `finalize()` adds 400 to every operation with
a body, query or path parameter.

### D11. Malformed-input rejections use the standard error body (infrastructure)

_Added during slice 1; user decision._ axum's `Json`, `Query` and `Path` reject
with plain text (and 415/422 for some JSON failures). Five routes used them
directly. New `ApiJson`, `ApiQuery` and `ApiPath` extractors
(`extractors/rejection.rs`) map every rejection to `AppError::Validation` (400,
`{code, message}`), as `ValidatedJson` already did. This is the change's one
deliberate behaviour change, and it only affects malformed requests. It was
chosen over documenting `text/plain` responses, because that would leave
clients with two error formats.

### D12. Not documented: the 408 timeout

`TimeoutLayer` answers 408 with an empty body. It isn't documented: it can
happen on any route, it carries no body for a client to read, and documenting it
as an `ErrorResponse` would be false. Revisit if the timeout ever returns a body.

## Risks / Trade-offs

- **[Risk] The router refactor changes middleware order or protection on some
  route.** → Mitigation: no group changes its layers, only how state is supplied.
  `just test-integration` (CSRF, auth, verified-email, fresh-session and
  admin-role tests drive the real router) must pass unchanged before anything
  else lands. Add one integration test that a demoted admin's old token is
  rejected, if `admin_integration.rs` lacks it, since that layer's wiring is the
  one that changes.
- **[Risk] A handler mounted in the wrong group gets the wrong path.** Router and
  spec would agree with each other, but both would be wrong. → Mitigation: the
  integration tests call absolute URLs, so they fail with 404. A path change also
  shows up in review as a diff in the committed `openapi.json`.
- **[Risk] An `openapi-typescript` or `utoipa` upgrade rewrites the generated
  files.** → Mitigation: the exact version is pinned, and the diff check makes
  any change visible. Dependabot PRs that touch them must include regenerated
  files (the check fails otherwise).
- **[Trade-off] Two committed generated files mean merge conflicts on parallel
  branches.** → Resolve by regenerating (`just api-types`), never by hand.
- **[Trade-off] `disallowed-methods` is crate-wide.** Any future test-only
  router needs an `expect` with a reason. That's acceptable, and it's
  self-documenting.

## Migration Plan

No schema or data changes. One deploy, with no visible change: the API's
behaviour is identical. To roll back, revert the commit. Nothing outside the repo
depends on the committed files.

## Open Questions

- Should authenticated routes declare a `security` scheme (cookie session +
  CSRF header) in the spec? It's useful for the Swagger UI, but doesn't affect the
  generated types or any scenario. It can wait for a follow-up.
