//! Crypto primitives on the request path: session JWTs, CSRF tokens, OAuth
//! state, the at-rest secret cipher, opaque tokens, and Argon2.
//!
//! Argon2 runs with the production parameters from `.env.example`, so a
//! regression there is a real change in login/signup latency, not a test-key
//! artefact. Everything else uses the shared test keypair (same key size as
//! production: RSA-2048).

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use overkill_todo::application::ports::PasswordHasher;
use overkill_todo::infrastructure::api::{
    create_jwt, create_oauth_state, generate_csrf_token, verify_csrf_token, verify_jwt,
    verify_oauth_state,
};
use overkill_todo::infrastructure::config::CryptoSettings;
use overkill_todo::infrastructure::crypto::{Crypto, generate_secure_token, sha256_hex};
use overkill_todo::infrastructure::services::Argon2Hasher;
use uuid::Uuid;

/// Production Argon2id cost (`backend/.env.example`). Keep in sync if the
/// deployed parameters change, or the benchmark stops meaning "real login cost".
const PROD_ARGON2_MEMORY_KIB: u32 = 65536;
const PROD_ARGON2_TIME: u32 = 3;
const PROD_ARGON2_LANES: u32 = 4;

fn prod_argon2_crypto() -> Arc<Crypto> {
    let settings = CryptoSettings {
        argon2_memory_cost: PROD_ARGON2_MEMORY_KIB,
        argon2_time_cost: PROD_ARGON2_TIME,
        argon2_parallelism: PROD_ARGON2_LANES,
        jwt_private_key: common::TEST_PRIVATE_KEY.to_string(),
        jwt_public_key: common::TEST_PUBLIC_KEY.to_string(),
        jwt_expiration_minutes: 15,
        refresh_token_expiration_days: 30,
        csrf_secret_b64: "MWq42LqA6RbYEsgrc0w2Ik9YydReTlRwh_D5Ynus2QA".to_string(),
        secrets_encryption_key_b64: "MWq42LqA6RbYEsgrc0w2Ik9YydReTlRwh_D5Ynus2QA".to_string(),
    };
    Arc::new(Crypto::from_settings(&settings).expect("valid bench crypto settings"))
}

fn jwt(c: &mut Criterion) {
    let crypto = common::ensure_test_crypto();
    let user_id = Uuid::now_v7();
    let token = create_jwt(user_id, true, "user", crypto.encoding_key(), 15).unwrap();

    let mut g = c.benchmark_group("jwt");
    g.bench_function("create", |b| {
        b.iter(|| create_jwt(black_box(user_id), true, "user", crypto.encoding_key(), 15));
    });
    // Runs on every authenticated request (auth middleware).
    g.bench_function("verify", |b| {
        b.iter(|| verify_jwt(black_box(&token), crypto.decoding_key()));
    });
    // A well-formed signature from another token, so the full RSA check runs
    // (a corrupted base64 tail would fail fast and measure the wrong path).
    let other = create_jwt(Uuid::now_v7(), true, "user", crypto.encoding_key(), 15).unwrap();
    let forged = format!(
        "{}.{}",
        token.rsplit_once('.').unwrap().0,
        other.rsplit_once('.').unwrap().1
    );
    g.bench_function("verify_bad_signature", |b| {
        b.iter(|| verify_jwt(black_box(&forged), crypto.decoding_key()));
    });
    g.finish();
}

fn csrf(c: &mut Criterion) {
    let crypto = common::ensure_test_crypto();
    let user_id = Uuid::now_v7().to_string();
    let token = generate_csrf_token(crypto.csrf_secret(), &user_id);

    let mut g = c.benchmark_group("csrf");
    g.bench_function("generate", |b| {
        b.iter(|| generate_csrf_token(crypto.csrf_secret(), black_box(&user_id)));
    });
    // Runs on every mutating request (CSRF middleware).
    g.bench_function("verify", |b| {
        b.iter(|| {
            verify_csrf_token(
                black_box(&token),
                crypto.csrf_secret(),
                &user_id,
                crypto.csrf_lifetime_secs(),
            )
        });
    });
    g.finish();
}

fn oauth_state(c: &mut Criterion) {
    let crypto = common::ensure_test_crypto();
    let (state, _) = create_oauth_state(crypto.encoding_key(), "google", "login", None).unwrap();

    let mut g = c.benchmark_group("oauth_state");
    g.bench_function("create", |b| {
        b.iter(|| create_oauth_state(crypto.encoding_key(), "google", "login", None));
    });
    g.bench_function("verify", |b| {
        b.iter(|| verify_oauth_state(crypto.decoding_key(), black_box(&state)));
    });
    g.finish();
}

fn secret_cipher(c: &mut Criterion) {
    let crypto = common::ensure_test_crypto();
    let aad = Uuid::now_v7();
    let plaintext = "ya29.a0AfB_byC-oauth-refresh-token-shaped-secret-0123456789abcdef";
    let stored = crypto.encrypt_secret(plaintext, aad.as_bytes()).unwrap();

    let mut g = c.benchmark_group("secret_cipher");
    g.bench_function("encrypt", |b| {
        b.iter(|| crypto.encrypt_secret(black_box(plaintext), aad.as_bytes()));
    });
    g.bench_function("decrypt", |b| {
        b.iter(|| crypto.decrypt_secret(black_box(&stored), aad.as_bytes()));
    });
    g.finish();
}

fn opaque_tokens(c: &mut Criterion) {
    // Refresh tokens and verification links are `generate_secure_token` values,
    // stored and looked up by `sha256_hex`.
    let refresh = generate_secure_token();

    let mut g = c.benchmark_group("opaque_token");
    g.bench_function("generate_secure_token", |b| b.iter(generate_secure_token));
    g.bench_function("sha256_hex", |b| b.iter(|| sha256_hex(black_box(&refresh))));
    g.finish();
}

fn argon2(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let hasher = Argon2Hasher::new(prod_argon2_crypto());
    let password = "correct horse battery staple";
    let hash = rt.block_on(hasher.hash(password)).unwrap();

    let mut g = c.benchmark_group("argon2_prod_params");
    // Each iteration is ~100 ms+; criterion's minimum sample count keeps the
    // run to a few seconds while still detecting a cost change.
    g.sample_size(10);
    g.bench_function("hash", |b| {
        b.to_async(&rt).iter(|| hasher.hash(black_box(password)));
    });
    g.bench_function("verify_ok", |b| {
        b.to_async(&rt)
            .iter(|| hasher.verify(black_box(password), &hash));
    });
    g.bench_function("verify_wrong", |b| {
        b.to_async(&rt)
            .iter(|| hasher.verify(black_box("wrong password entirely"), &hash));
    });
    g.finish();
}

criterion_group!(
    benches,
    jwt,
    csrf,
    oauth_state,
    secret_cipher,
    opaque_tokens,
    argon2
);
criterion_main!(benches);
