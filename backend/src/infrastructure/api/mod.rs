//! HTTP API layer — handlers, middleware, extractors, router.
//!
//! This layer also owns the HTTP-facing error type ([`error::AppError`], with its
//! `IntoResponse`/`From` mappings) and the JWT / OAuth-state helpers that produce
//! it — they import axum/sqlx/jsonwebtoken, so they belong here rather than in the
//! dependency-light `shared` leaf.

pub mod cookies;
pub mod csrf;
pub mod error;
pub mod extractors;
pub mod handlers;
pub mod jwt;
pub mod middleware;
pub mod oauth_state;
pub mod openapi;
pub mod responses;
pub mod routes;

pub use csrf::{generate_csrf_token, verify_csrf_token};
pub use error::AppError;
pub use jwt::{Claims, create_jwt, verify_jwt};
pub use oauth_state::{create_oauth_state, verify_oauth_state};
pub use routes::{RouterDeps, create_router};
