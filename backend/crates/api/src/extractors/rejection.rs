//! `Json`, `Query` and `Path` extractors whose rejections use the API's error body.
//!
//! axum's own extractors reject a malformed request with a plain-text body
//! (and 415/422 for some JSON failures). These wrappers turn every rejection into
//! [`AppError::Validation`], so clients always get `400` with the standard
//! `{ code, message }` body that the `OpenAPI` description promises. Use them
//! (or [`super::ValidatedJson`]) in handlers instead of the bare axum types.

use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Query, Request, rejection::JsonRejection},
    http::{StatusCode, request::Parts},
};
use serde::de::DeserializeOwned;

use crate::error::AppError;

/// JSON body without `validator` rules.
///
/// Prefer [`super::ValidatedJson`] when the type has constraints. A body over
/// the route's `DefaultBodyLimit` is a `413` ([`AppError::PayloadTooLarge`]);
/// every other rejection is the JSON `400`.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(req, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|e| match e {
                JsonRejection::BytesRejection(b) if b.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                    AppError::PayloadTooLarge
                }
                e => AppError::Validation(format!("Invalid JSON body: {e}")),
            })
    }
}

/// Query string.
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|e| AppError::Validation(format!("Invalid query string: {e}")))
    }
}

/// Path parameters.
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|e| AppError::Validation(format!("Invalid path parameter: {e}")))
    }
}
