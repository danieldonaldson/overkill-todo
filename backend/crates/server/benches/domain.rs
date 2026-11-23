//! Pure CPU work outside crypto: value-object parsing, request DTO
//! deserialisation + validation, error fingerprinting, error → response
//! mapping, and email rendering.

use std::hint::black_box;

use axum::response::IntoResponse;
use criterion::{Criterion, criterion_group, criterion_main};
use overkill_todo::application::dto::validation::sanitize_verification_code;
use overkill_todo::application::dto::{
    CompleteRegistrationRequest, LoginRequest, VerifyEmailRequest,
};
use overkill_todo::domain::errors::DomainError;
use overkill_todo::domain::value_objects::{Email, UserRole};
use overkill_todo::infrastructure::api::AppError;
use overkill_todo::infrastructure::services::email_templates::{
    password_reset_email, verification_email,
};
use overkill_todo::shared::fingerprint::generate_error_fingerprint;
use validator::Validate;

fn value_objects(c: &mut Criterion) {
    let mut g = c.benchmark_group("value_objects");
    g.bench_function("email_valid", |b| {
        b.iter(|| Email::new(black_box("  Alice.Example+todo@Example.COM ")));
    });
    g.bench_function("email_invalid", |b| {
        b.iter(|| Email::new(black_box("not-an-email@")));
    });
    g.bench_function("role_from_db_str", |b| {
        b.iter(|| UserRole::from_db_str(black_box("super_admin")));
    });
    g.bench_function("sanitize_verification_code", |b| {
        b.iter(|| sanitize_verification_code(black_box(" 123-456 ")));
    });
    g.finish();
}

fn request_dtos(c: &mut Criterion) {
    let login = br#"{"email":"alice@example.com","password":"correct horse battery staple"}"#;
    let verify = br#"{"email":"alice@example.com","code":"123456"}"#;
    let complete = br#"{"signup_token":"eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ4In0.c2ln","email":"alice@example.com","password":"correct horse battery staple"}"#;

    // What axum's `Json` extractor + `.validate()` cost per request.
    let mut g = c.benchmark_group("request_dto");
    g.bench_function("login", |b| {
        b.iter(|| {
            let req: LoginRequest = serde_json::from_slice(black_box(login)).unwrap();
            req.validate()
        });
    });
    g.bench_function("verify_email", |b| {
        b.iter(|| {
            let req: VerifyEmailRequest = serde_json::from_slice(black_box(verify)).unwrap();
            req.validate()
        });
    });
    g.bench_function("complete_signup", |b| {
        b.iter(|| {
            let req: CompleteRegistrationRequest =
                serde_json::from_slice(black_box(complete)).unwrap();
            req.validate()
        });
    });
    g.finish();
}

fn fingerprint(c: &mut Criterion) {
    let short = "connection refused";
    let noisy = "query failed for user 550e8400-e29b-41d4-a716-446655440000 at \
                 2026-11-07T12:34:56.789Z: connection to 192.168.1.100:5432 timed out \
                 after 30s (request 123456789, retry 3 of 5)";
    let long = noisy.repeat(20);

    let mut g = c.benchmark_group("error_fingerprint");
    g.bench_function("short", |b| {
        b.iter(|| generate_error_fingerprint("Database", black_box(short)));
    });
    g.bench_function("noisy", |b| {
        b.iter(|| generate_error_fingerprint("Database", black_box(noisy)));
    });
    // Guards against accidentally quadratic normalisation on big messages.
    g.bench_function("long_4k", |b| {
        b.iter(|| generate_error_fingerprint("Database", black_box(&long)));
    });
    g.finish();
}

fn error_responses(c: &mut Criterion) {
    let mut g = c.benchmark_group("error_response");
    g.bench_function("domain_not_found", |b| {
        b.iter(|| {
            AppError::from(DomainError::NotFound(black_box("user 42").to_string())).into_response()
        });
    });
    // Internal errors are logged and fingerprinted before being hidden.
    g.bench_function("internal", |b| {
        b.iter(|| {
            AppError::Internal(black_box("pool timed out after 30s").to_string()).into_response()
        });
    });
    g.bench_function("csrf_invalid", |b| {
        b.iter(|| AppError::csrf_invalid().into_response());
    });
    g.finish();
}

fn email_templates(c: &mut Criterion) {
    let link = "https://app.example.com/verify-magic?token=0123456789abcdef0123456789abcdef";

    let mut g = c.benchmark_group("email_template");
    g.bench_function("verification_with_link", |b| {
        b.iter(|| verification_email("Overkill Todo", black_box("Alice"), "123456", Some(link)));
    });
    g.bench_function("password_reset", |b| {
        b.iter(|| password_reset_email("Overkill Todo", black_box("Alice"), "123456"));
    });
    g.finish();
}

criterion_group!(
    benches,
    value_objects,
    request_dtos,
    fingerprint,
    error_responses,
    email_templates
);
criterion_main!(benches);
