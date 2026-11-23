//! Validated JSON extractor with validator support.

use axum::{
    Json,
    extract::{FromRequest, Request},
};
use serde::de::DeserializeOwned;
use validator::Validate;

use crate::infrastructure::api::error::AppError;

pub struct ValidatedJson<T>(pub T);

impl<T, S> FromRequest<S> for ValidatedJson<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state).await.map_err(|e| {
            tracing::error!("JSON parsing error: {:?}", e);
            AppError::Validation(format!("Invalid JSON body: {e}"))
        })?;

        value.validate().map_err(|e| {
            tracing::error!("Validation error: {:?}", e);
            AppError::Validation(e.to_string())
        })?;

        Ok(Self(value))
    }
}
