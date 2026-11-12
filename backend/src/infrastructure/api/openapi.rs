//! `OpenAPI` description of the HTTP API.
//!
//! Paths and their schemas are not listed here: [`super::routes::api_routes`]
//! mounts each handler with `routes!`, which adds its `#[utoipa::path]` and the
//! schemas it references. This module only holds the document-level metadata
//! ([`ApiDoc`]) and [`openapi`], which `examples/export_openapi.rs` writes to
//! `backend/openapi.json` (`just api-spec`).

use utoipa::OpenApi;
use utoipa::openapi::{Content, Ref, RefOr, Response, path::Operation};

use crate::application::dto::OAuthMode;
use crate::infrastructure::api::error::ErrorResponse;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Overkill Todo API",
        version = "1.0.0",
        description = "Backend API"
    ),
    // `routes!` collects body schemas but not types named only in `params(...)`
    // tuples, nor `ErrorResponse`, which `finalize` attaches to error responses.
    // `every_schema_ref_resolves` fails if one is missed.
    components(schemas(OAuthMode, ErrorResponse)),
    tags(
        (name = "Authentication", description = "Auth endpoints"),
        (name = "Verification", description = "Email verification"),
        (name = "OAuth", description = "OAuth providers"),
        (name = "Profile", description = "Profile management"),
        (name = "Admin", description = "Admin panel"),
        (name = "Health", description = "Liveness and readiness probes"),
    )
)]
pub struct ApiDoc;

/// Call `f(method, path, operation)` for every operation in `spec`.
pub(crate) fn for_each_operation(
    spec: &mut utoipa::openapi::OpenApi,
    mut f: impl FnMut(&str, &str, &mut Operation),
) {
    for (path, item) in &mut spec.paths.paths {
        for (method, op) in [
            ("GET", &mut item.get),
            ("PUT", &mut item.put),
            ("POST", &mut item.post),
            ("DELETE", &mut item.delete),
            ("PATCH", &mut item.patch),
            ("OPTIONS", &mut item.options),
            ("HEAD", &mut item.head),
            ("TRACE", &mut item.trace),
        ] {
            if let Some(op) = op {
                f(method, path, op);
            }
        }
    }
}

/// Declare `status` on `op` unless its handler annotation already does (a
/// handler's own, more specific description wins).
pub(crate) fn add_response_if_missing(op: &mut Operation, status: &str, description: &str) {
    op.responses
        .responses
        .entry(status.to_string())
        .or_insert_with(|| RefOr::T(Response::new(description)));
}

/// Document-wide post-processing, applied to every published copy of the spec
/// (the exported file and the dev-only Swagger UI).
#[must_use]
pub fn finalize(mut spec: utoipa::openapi::OpenApi) -> utoipa::openapi::OpenApi {
    for_each_operation(&mut spec, |_, _, op| {
        // Every body, query and path extractor rejects malformed input with 400
        // (`ValidatedJson`, `ApiJson`, `ApiQuery`, `ApiPath`).
        let takes_input =
            op.request_body.is_some() || op.parameters.as_ref().is_some_and(|p| !p.is_empty());
        if takes_input {
            add_response_if_missing(op, "400", "Malformed or invalid request");
        }
        // Any route can fail internally (database, external service).
        add_response_if_missing(op, "500", "Internal error");

        // Errors that don't name a body carry the standard `AppError` body.
        for (status, response) in &mut op.responses.responses {
            let is_error = status.starts_with('4') || status.starts_with('5');
            if let (true, RefOr::T(response)) = (is_error, response)
                && response.content.is_empty()
            {
                response.content.insert(
                    "application/json".to_string(),
                    RefOr::T(Content::new(Some(Ref::from_schema_name("ErrorResponse")))),
                );
            }
        }
    });
    spec
}

/// The complete `OpenAPI` description, built from the route tree alone (no
/// database, environment or adapters needed).
#[must_use]
pub fn openapi() -> utoipa::openapi::OpenApi {
    finalize(super::routes::api_routes().into_openapi())
}

#[cfg(test)]
mod tests {
    use super::openapi;
    use std::collections::BTreeSet;

    fn operations() -> BTreeSet<(String, String)> {
        let spec = serde_json::to_value(openapi()).unwrap();
        spec["paths"]
            .as_object()
            .unwrap()
            .iter()
            .flat_map(|(path, item)| {
                item.as_object()
                    .unwrap()
                    .keys()
                    .map(move |method| (method.to_uppercase(), path.clone()))
            })
            .collect()
    }

    /// Routes that were served but undocumented before the description was
    /// derived from the router (OT-002) are now described.
    #[test]
    fn previously_undocumented_routes_are_described() {
        let ops = operations();
        for (method, path) in [
            ("POST", "/api/v1/auth/forgot-password"),
            ("POST", "/api/v1/auth/verify-reset-code"),
            ("POST", "/api/v1/auth/reset-password"),
            ("GET", "/api/v1/auth/verify-magic"),
            ("DELETE", "/api/v1/auth/oauth/{provider}/unlink"),
            ("GET", "/api/v1/health"),
            ("GET", "/api/v1/health/ready"),
        ] {
            assert!(
                ops.contains(&(method.to_string(), path.to_string())),
                "{method} {path} missing from the OpenAPI description"
            );
        }
    }

    /// Every operation lives under `/api/v1`: the root-level health probes are a
    /// deliberate, undocumented duplicate for the container HEALTHCHECK.
    #[test]
    fn every_path_is_under_api_v1() {
        let stray: Vec<_> = operations()
            .into_iter()
            .filter(|(_, path)| !path.starts_with("/api/v1/"))
            .collect();
        assert!(stray.is_empty(), "paths outside /api/v1: {stray:?}");
    }

    #[test]
    fn operation_ids_are_unique() {
        let spec = serde_json::to_value(openapi()).unwrap();
        let mut seen = BTreeSet::new();
        for item in spec["paths"].as_object().unwrap().values() {
            for op in item.as_object().unwrap().values() {
                let id = op["operationId"].as_str().unwrap().to_string();
                assert!(seen.insert(id.clone()), "duplicate operationId {id}");
            }
        }
    }

    /// Every `$ref` points at a schema that is actually in `components`, so the
    /// automatic schema collection hasn't missed a nested type.
    #[test]
    fn every_schema_ref_resolves() {
        fn refs(value: &serde_json::Value, out: &mut BTreeSet<String>) {
            match value {
                serde_json::Value::Object(map) => {
                    if let Some(serde_json::Value::String(r)) = map.get("$ref") {
                        out.insert(r.clone());
                    }
                    map.values().for_each(|v| refs(v, out));
                }
                serde_json::Value::Array(items) => items.iter().for_each(|v| refs(v, out)),
                _ => {}
            }
        }
        let spec = serde_json::to_value(openapi()).unwrap();
        let mut found = BTreeSet::new();
        refs(&spec, &mut found);
        let schemas = spec["components"]["schemas"].as_object().unwrap();
        let missing: Vec<_> = found
            .iter()
            .filter(|r| {
                r.strip_prefix("#/components/schemas/")
                    .is_none_or(|name| !schemas.contains_key(name))
            })
            .collect();
        assert!(missing.is_empty(), "unresolved $refs: {missing:?}");
    }

    fn statuses(method: &str, path: &str) -> BTreeSet<String> {
        let spec = serde_json::to_value(openapi()).unwrap();
        spec["paths"][path][method.to_lowercase()]["responses"]
            .as_object()
            .unwrap_or_else(|| panic!("{method} {path} not described"))
            .keys()
            .cloned()
            .collect()
    }

    fn assert_statuses(method: &str, path: &str, expected: &[&str]) {
        let found = statuses(method, path);
        for status in expected {
            assert!(
                found.contains(*status),
                "{method} {path} should document {status}; has {found:?}"
            );
        }
    }

    /// The route-group guards document the statuses their middleware returns.
    #[test]
    fn guards_document_their_statuses() {
        // auth only
        assert_statuses("GET", "/api/v1/auth/me", &["401"]);
        // auth + CSRF (mutating, not excluded)
        assert_statuses(
            "DELETE",
            "/api/v1/auth/oauth/{provider}/unlink",
            &["401", "403"],
        );
        // auth + verified email (+ CSRF on mutating routes)
        assert_statuses("GET", "/api/v1/users/me/export", &["401", "403"]);
        assert_statuses("POST", "/api/v1/users/password/change", &["401", "403"]);
        // auth + fresh session + verified email
        assert_statuses("GET", "/api/v1/admin/users", &["401", "403"]);
    }

    /// CSRF-excluded and read-only routes don't claim a CSRF 403.
    #[test]
    fn csrf_403_only_where_csrf_applies() {
        assert!(!statuses("POST", "/api/v1/auth/login").contains("403"));
        assert!(!statuses("POST", "/api/v1/auth/forgot-password").contains("403"));
        assert!(!statuses("GET", "/api/v1/auth/me").contains("403"));
        assert!(!statuses("GET", "/api/v1/health").contains("403"));
    }

    /// Routes that take a body, query or path parameter document the 400 their
    /// extractors return for malformed input.
    #[test]
    fn input_taking_routes_document_400() {
        assert_statuses("POST", "/api/v1/auth/login", &["400"]);
        assert_statuses("GET", "/api/v1/auth/verify-magic", &["400"]);
        assert_statuses("PUT", "/api/v1/admin/users/{id}/role", &["400"]);
        assert!(!statuses("GET", "/api/v1/health").contains("400"));
    }

    fn schema(name: &str) -> serde_json::Value {
        let spec = serde_json::to_value(openapi()).unwrap();
        spec["components"]["schemas"][name].clone()
    }

    fn is_required(schema: &serde_json::Value, field: &str) -> bool {
        schema["required"]
            .as_array()
            .is_some_and(|r| r.iter().any(|f| f == field))
    }

    /// A field is nullable if its type list (3.1) includes `"null"`.
    fn is_nullable(schema: &serde_json::Value, field: &str) -> bool {
        let ty = &schema["properties"][field]["type"];
        ty == "null" || ty.as_array().is_some_and(|t| t.iter().any(|v| v == "null"))
    }

    /// Optional request fields (serde accepts them missing) aren't required.
    #[test]
    fn optional_request_fields_are_not_required() {
        for name in [
            "LoginRequest",
            "InitiateRegistrationRequest",
            "ResendVerificationRequest",
            "ForgotPasswordRequest",
        ] {
            assert!(
                !is_required(&schema(name), "turnstile_token"),
                "{name}.turnstile_token should be optional"
            );
        }
        let deletion = schema("RequestDeletionRequest");
        assert!(!is_required(&deletion, "password"));
        assert!(!is_required(&deletion, "confirmation"));
    }

    /// Response fields omitted when empty are optional; always-sent fields are
    /// required and not nullable.
    #[test]
    fn response_field_presence_matches_serialization() {
        let user = schema("UserResponse");
        for field in [
            "pending_email",
            "pending_email_requested_at",
            "deletion_scheduled_at",
        ] {
            assert!(
                !is_required(&user, field),
                "UserResponse.{field} is omitted when empty"
            );
            // Omitted, never sent as `null`.
            assert!(
                !is_nullable(&user, field),
                "UserResponse.{field} is never null"
            );
        }
        for field in ["id", "email", "email_verified", "role", "oauth_providers"] {
            assert!(
                is_required(&user, field),
                "UserResponse.{field} is always sent"
            );
            assert!(
                !is_nullable(&user, field),
                "UserResponse.{field} is never null"
            );
        }
    }

    fn enum_values(schema: &serde_json::Value) -> Vec<String> {
        schema["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("not an enum schema: {schema}"))
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    fn resolve(reference: &serde_json::Value) -> serde_json::Value {
        let name = reference["$ref"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a $ref: {reference}"))
            .trim_start_matches("#/components/schemas/");
        schema(name)
    }

    /// Fixed-value string fields list their values instead of plain `string`.
    #[test]
    fn fixed_value_fields_list_their_values() {
        let roles = ["user", "admin", "super_admin"];
        for (owner, field) in [
            ("UserResponse", "role"),
            ("AdminUserResponse", "role"),
            ("SetUserRoleRequest", "role"),
            ("AccountExportResponse", "role"),
        ] {
            let role = resolve(&schema(owner)["properties"][field]);
            assert_eq!(enum_values(&role), roles, "{owner}.{field}");
        }
        let providers = resolve(&schema("UserResponse")["properties"]["oauth_providers"]["items"]);
        assert_eq!(enum_values(&providers), ["google", "apple"]);
        let linked = resolve(&schema("LinkedOAuthAccountExport")["properties"]["provider"]);
        assert_eq!(enum_values(&linked), ["google", "apple"]);
    }

    /// Every documented error names a JSON body, so clients always know the
    /// shape of an error.
    #[test]
    fn every_error_response_has_a_json_body() {
        let spec = serde_json::to_value(openapi()).unwrap();
        let mut missing = Vec::new();
        for (path, item) in spec["paths"].as_object().unwrap() {
            for (method, op) in item.as_object().unwrap() {
                for (status, response) in op["responses"].as_object().unwrap() {
                    let is_error = status.starts_with('4') || status.starts_with('5');
                    if is_error && response["content"]["application/json"]["schema"].is_null() {
                        missing.push(format!("{method} {path} {status}"));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "error responses without a body: {missing:?}"
        );
    }

    #[test]
    fn every_operation_documents_500() {
        let missing: Vec<_> = operations()
            .into_iter()
            .filter(|(m, p)| !statuses(m, p).contains("500"))
            .collect();
        assert!(missing.is_empty(), "no 500 documented: {missing:?}");
    }

    #[test]
    fn standard_error_body_requires_code_and_message() {
        let error = schema("ErrorResponse");
        assert!(is_required(&error, "code"));
        assert!(is_required(&error, "message"));
        let spec = serde_json::to_value(openapi()).unwrap();
        assert_eq!(
            spec["paths"]["/api/v1/auth/me"]["get"]["responses"]["401"]["content"]["application/json"]
                ["schema"]["$ref"],
            "#/components/schemas/ErrorResponse"
        );
    }

    /// verify-email's 400 can be either the verification error (wrong, expired
    /// or locked code) or the standard error (malformed request).
    #[test]
    fn verify_email_400_names_the_verification_body() {
        let spec = serde_json::to_value(openapi()).unwrap();
        let body = resolve(
            &spec["paths"]["/api/v1/auth/verify-email"]["post"]["responses"]["400"]["content"]["application/json"]
                ["schema"],
        );
        let variants: Vec<_> = body["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("expected oneOf: {body}"))
            .iter()
            .map(|v| v["$ref"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            variants,
            [
                "#/components/schemas/VerificationErrorResponse",
                "#/components/schemas/ErrorResponse"
            ]
        );
        let verification = schema("VerificationErrorResponse");
        assert!(!is_required(&verification, "attempts_remaining"));
        assert!(!is_required(&verification, "cooldown_seconds"));
    }

    /// The description never carries internal error text or example traces.
    #[test]
    fn description_has_no_internal_error_text() {
        let json = openapi().to_pretty_json().unwrap();
        for leak in [
            "Database error:",
            "Internal error:",
            "stack backtrace",
            "panicked at",
        ] {
            assert!(!json.contains(leak), "spec contains {leak:?}");
        }
    }

    #[test]
    fn export_is_deterministic() {
        assert_eq!(
            openapi().to_pretty_json().unwrap(),
            openapi().to_pretty_json().unwrap()
        );
    }
}
