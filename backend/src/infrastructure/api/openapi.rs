//! `OpenAPI` spec. Register every new handler and DTO here.

use utoipa::OpenApi;

use crate::application::dto::{
    AccountDeletionResponse, AccountExportResponse, AdminUserListResponse, AdminUserResponse,
    AuthResponse, CompleteRegistrationRequest, InitiateRegistrationRequest,
    InitiateRegistrationResponse, LinkedOAuthAccountExport, LoginRequest,
    OAuthDeviceFingerprintRequest, OAuthMode, OAuthStartQuery, RequestDeletionRequest,
    SetUserRoleRequest, UserResponse, VerifyEmailResponse, VerifyMagicLinkQuery,
};
use crate::infrastructure::api::handlers::{
    admin_handler, auth_handler, oauth_handler, profile_handler, verification_handler,
};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "App API",
        version = "1.0.0",
        description = "Backend API"
    ),
    paths(
        auth_handler::register,
        auth_handler::login,
        auth_handler::logout,
        auth_handler::refresh,
        auth_handler::me,
        verification_handler::verify_email,
        verification_handler::complete_signup,
        verification_handler::resend_verification,
        oauth_handler::oauth_store_device_fingerprint,
        oauth_handler::oauth_start,
        oauth_handler::oauth_callback,
        profile_handler::change_password,
        profile_handler::set_password,
        profile_handler::export_data,
        profile_handler::request_deletion,
        profile_handler::cancel_deletion,
        admin_handler::list_users,
        admin_handler::set_user_role,
    ),
    components(schemas(
        InitiateRegistrationRequest,
        InitiateRegistrationResponse,
        LoginRequest,
        AuthResponse,
        UserResponse,
        CompleteRegistrationRequest,
        VerifyEmailResponse,
        VerifyMagicLinkQuery,
        OAuthDeviceFingerprintRequest,
        OAuthStartQuery,
        OAuthMode,
        RequestDeletionRequest,
        AccountExportResponse,
        LinkedOAuthAccountExport,
        AccountDeletionResponse,
        AdminUserResponse,
        AdminUserListResponse,
        SetUserRoleRequest,
    )),
    tags(
        (name = "Authentication", description = "Auth endpoints"),
        (name = "Verification", description = "Email verification"),
        (name = "OAuth", description = "OAuth providers"),
        (name = "Profile", description = "Profile management"),
        (name = "Admin", description = "Admin panel"),
    )
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::ApiDoc;
    use crate::infrastructure::api::routes::MOUNTED_ROUTE_TEMPLATES;
    use std::collections::HashSet;
    use utoipa::OpenApi;

    /// Every `#[utoipa::path(... path = ...)]` string in the `OpenAPI` document must
    /// correspond to a route actually mounted by `create_router`. This guards the
    /// class of regression where the profile handler still declared the
    /// pre-v1-rename `/api/users/password/*` paths while the router served
    /// `/api/v1/users/password/*`.
    #[test]
    fn every_openapi_path_is_routable() {
        let doc = ApiDoc::openapi();
        let mounted: HashSet<&str> = MOUNTED_ROUTE_TEMPLATES.iter().copied().collect();

        let unroutable: Vec<String> = doc
            .paths
            .paths
            .keys()
            .filter(|path| !mounted.contains(path.as_str()))
            .cloned()
            .collect();

        assert!(
            unroutable.is_empty(),
            "OpenAPI declares paths that are not mounted by create_router: {unroutable:?}. \
             Fix the #[utoipa::path(path = ...)] string or update MOUNTED_ROUTE_TEMPLATES."
        );
    }
}
