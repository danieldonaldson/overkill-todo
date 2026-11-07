//! API response helpers.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

pub struct ApiResponse<T: Serialize> {
    pub status: StatusCode,
    pub data: T,
}

impl<T: Serialize> ApiResponse<T> {
    pub const fn ok(data: T) -> Self {
        Self {
            status: StatusCode::OK,
            data,
        }
    }
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    fn into_response(self) -> Response {
        (self.status, Json(self.data)).into_response()
    }
}
