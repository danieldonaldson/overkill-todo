# Design

## Context

See proposal.md (Why) and specs/security/spec.md for the requirements. What
the code does today:

- **Tokens.** `infrastructure/crypto.rs` loads an RSA pair from
  `JWT_PRIVATE_KEY` / `JWT_PUBLIC_KEY`. Three places sign RS256 with
  `jsonwebtoken`: `infrastructure/api/jwt.rs` (session tokens, 15 min, in the
  `auth_token` HttpOnly cookie), `infrastructure/services/jwt_service.rs`
  (signup and reset tokens behind the `JwtService` port) and
  `infrastructure/api/oauth_state.rs` (the OAuth `state` query parameter, with
  its nonce in the `oauth_state` cookie). Only the backend verifies them: there
  is no JWKS endpoint and no third-party verifier. The frontend never reads the
  token (it only tracks the expiry). Refresh tokens are opaque random values
  stored as SHA-256 hashes, so they need no change.
- **Google ID tokens** are verified with `jsonwebtoken` against Google's
  RS256 JWKS (`infrastructure/oauth/google.rs`). That stays as it is. Its
  tests borrow `Crypto`'s RSA key today, so they need their own test key.
- **TLS.** rustls 0.23.46 with the aws-lc-rs provider and its default
  `prefer-post-quantum` feature already offers X25519MLKEM768 first. reqwest
  (Google, Turnstile) uses rustls. The connections to Postgres, the OTel
  collector and Prometheus are plaintext on the single-host Docker network
  (out of scope). The local Caddy runs `auto_https off`, so there's no local
  edge TLS. The production edge (Traefik) isn't built yet.
- **Secrets.** Before slice 1, `deploy/secrets/access.yaml` held one
  classical `age1…` key and the only committed secret was
  `prod/grafana_admin_password.sops`. Backend secrets aren't in SOPS yet.
  Installed versions: age 1.3.1, SOPS 3.13.2.

## Goals / Non-Goals

**Goals:**
- One signing path in the backend for every token we issue, with no RS256
  left.
- Crypto agility: changing algorithm or key is configuration plus a small
  code change, never a rewrite.
- Each guarantee is enforced by the gate, not by memory.

**Non-Goals:**
- Tokens anyone other than the backend can verify.
- Server-side sessions. Tokens stay stateless so the dashboard's optimistic
  path doesn't gain a database lookup.

## Hypothesis

The riskiest part is now key handling, not the algorithm: `jsonwebtoken`
already implements HS512, and tokens get smaller than today. We believe a
keyring with one active key and named trusted keys gives rotation with no
sign-outs and no window where a retired key still works. Slice 2 tests this
with backend integration tests for both rotation scenarios, including the
silent refresh after a key is retired. There's no production yet, so the
flag-free cutover in slice 2 is safe.

**What slice 2 showed (2026-11-13).** Confirmed. The rotation integration
tests (`backend/tests/token_keys_integration.rs`) pass for all three cases: a
previous key still verifies while trusted and the next token names the new
key; a retired key's session gets 401 `token_invalid` and refresh issues a
new-key token; a retired key's reset link is refused and a new one works.
Measured against the running backend: a session token is 372 bytes with
header `{"alg":"HS512","kid":"dev"}` (RS256 was ≈0.6 KB; ML-DSA-44 would have
been ≈3.6 KB). Changes from review:
- One rejection message for every token kind (`TOKEN_REJECTED_MESSAGE`,
  "Invalid token"), including a session token whose subject isn't a UUID.
  Before this, signup/reset failures sent the decoder's error text to the
  client. Expired session tokens keep `token_expired`.
- Each token kind's `Validation` is built once (`LazyLock`). `verify` refuses
  any `Validation` that allows another algorithm instead of copying and
  overriding it.
- `gen-dev-env.sh` writes the key format the target checkout expects (HMAC,
  or the RSA pair for a worktree of a pre-OT-009 commit).
- The rotation wait is the longest token lifetime (signup, 30 min), not the
  session lifetime (D5 updated).
- Not done: the integration tests and benches build their own test keys and
  test `CryptoSettings` rather than sharing the library's `cfg(test)`
  helpers. Sharing them needs a `test-support` Cargo feature, which isn't
  worth adding for three call sites.
- Found, not fixed (outside this change): signup and reset failures are
  always `token_invalid`, never `token_expired`, so the frontend's "link
  expired" branches in `ResetPasswordForm` / `SetPasswordForm` never fire.
  That was true before slice 2 too.

## Decisions

**D1. HMAC-SHA-512 (JOSE `HS512`) with keys of at least 64 bytes.** A
symmetric MAC is quantum-safe: Grover's algorithm at most halves the
effective key length, leaving 256-bit security for a 512-bit key, and
forging a tag needs the key. HS512 rather than HS256 for margin, in line with
CNSA 2.0's preference for SHA-384/512. The cost is 43 more characters per
token. The whole token stays around 0.4 KB, smaller than today's RS256 token
(≈0.6 KB). Verification costs a few microseconds.

Alternatives:
- ML-DSA-44 (FIPS 204). Chosen on 2026-11-12, then dropped on 2026-11-13.
  Its tokens are ≈3.6 KB, which adds ≈3 KB to every request (≈2.4 Gbps
  inbound at 100k req/s, worse because the cookie is `Path=/`). Verification
  costs ≈50–100 µs per request. The only benefit, verification by a third
  party, doesn't apply.
- ML-DSA-65: doesn't fit in a cookie at all.
- Opaque server-side sessions: see Non-Goals.

**D2. Stay on `jsonwebtoken`; select the key by `kid` before verifying.**
Signing sets `Header { alg: HS512, kid: Some(active_kid), .. }`.
Verification:
1. `decode_header` reads `kid` without trusting anything else. A missing or
   unknown `kid` fails, and no other keys are tried.
2. `decode` runs with that key and `Validation::new(Algorithm::HS512)`.
   `jsonwebtoken` rejects any token whose `alg` isn't in the allowed list, so
   RS256, HS256 and `none` are refused even if correctly signed.
3. The existing `exp` (default 60 s leeway), `iss`, `aud`, required-claim
   and `typ` checks stay as they are.

One helper pair in `infrastructure/crypto.rs` (`TokenKeyring::sign` /
`TokenKeyring::verify`) does this for all four token kinds. Errors keep
mapping to the existing `AppError::token_*` / `DomainError` values, so 401
bodies are unchanged. This removes the plan for a hand-written JWS module.

**D3. One `TokenKeyring` on `Crypto`; the ports are unchanged.** `Crypto`
replaces its RSA `EncodingKey`/`DecodingKey` with a keyring: the active
`(kid, EncodingKey)` and a `kid → DecodingKey` map that includes the active
key. `jwt.rs`, `oauth_state.rs`, the auth middleware and `CryptoJwtService`
call it. The application-layer `JwtService` port and the domain don't
change. Every token kind is signed with the same key, as today. The `typ`
claim and the distinct claim shapes keep one kind from passing as another.
`jsonwebtoken` keeps doing Google ID tokens, and those tests get their own
RSA test key.

**D4. Key configuration and generation.**
- `JWT_SIGNING_KEY=<kid>:<base64 secret>` is the active key. It signs and
  is trusted.
- `JWT_TRUSTED_KEYS=<kid>:<base64 secret>,…` is optional: previous keys
  that are still accepted for verification. These are secrets too, because
  with HMAC a verification key can also sign. They live in SOPS beside the
  signing key.
- A secret decodes (url-safe or standard base64, like the other secrets) to
  at least 64 bytes. A `kid` is 1–16 characters from `[A-Za-z0-9_-]`, e.g.
  `2026-11a`. Names must be unique across both settings.
- `JWT_PRIVATE_KEY` / `JWT_PUBLIC_KEY` are removed. Startup fails if either
  is still set, so stale config is noticed.
- `CryptoInitError` gets one variant per spec failure. Messages name the
  setting and the `kid`, never key bytes.
- Startup logs the active and trusted `kid`s, never secrets.
- `just jwt-keygen [kid]` prints `<kid>:<64 random bytes, base64url>` using
  `openssl rand`, with no backend subcommand. `scripts/gen-dev-env.sh` uses
  the same command.

**D5. Rotation.** To rotate:
1. Generate a new key.
2. Move the current `JWT_SIGNING_KEY` into `JWT_TRUSTED_KEYS`, set the new
   one as `JWT_SIGNING_KEY`, and deploy.
3. Once the longest-lived token the old key signed has expired, plus 60 s
   leeway, remove the old key from `JWT_TRUSTED_KEYS` and deploy. Today that
   is the 30-minute signup token (sessions 15 min, reset tokens and OAuth
   state 5 min).

A Member whose token is rejected after step 3 gets a 401. The dashboard's
`HttpClient` refreshes with the opaque refresh token, which isn't signed, so
they stay signed in.

**D6. Backend TLS: make the default explicit and test it.** At startup,
`main.rs` installs the aws-lc-rs provider as the process default with
`kx_groups` starting at `X25519MLKEM768`, rather than relying on a Cargo
feature default. A test starts an in-process rustls server that accepts
*only* X25519MLKEM768 and connects with the same reqwest client builder the
adapters use. If the hybrid is ever dropped, the handshake fails. The test
lives in `backend/tests/`, so it runs in `just test-integration`.
Implemented in slice 3 (`infrastructure/tls.rs`, `tests/tls_key_exchange.rs`).
reqwest (Google, Turnstile) and lettre (SMTP) both take rustls's process
default provider, so the one install covers all three. Removing the hybrid
from the provider makes the hybrid-only tests fail, which was checked by
doing it. The test server is a plain rustls server on a thread with an
embedded self-signed certificate, so no new dependencies were needed.

**D7. Edge TLS: a proxy-agnostic contract.** The contract: whatever
terminates public TLS offers X25519MLKEM768 first and allows classical
fallback. For Traefik (Go ≥ 1.24), that means not setting
`curvePreferences`, or listing `X25519MLKEM768` first. The check is a new
read-only smoke test in `just smoke <url>`: for an `https` URL, connect with
`openssl s_client -groups X25519MLKEM768` (OpenSSL ≥ 3.5, pinned in
`.mise.toml` if needed) and fail unless it's negotiated. Locally there's no
edge TLS, so this check runs only against deployed URLs, which is a
first-deploy note. Implemented as `scripts/smoke-tls.sh`, run first by
`just smoke`. It passed against cloudflare.com and failed against two hosts
without the hybrid (github.com, gnu.org) on 2026-11-13. OpenSSL reports a
classical group as `Peer Temp Key` (3.5+) or `Server Temp Key` (older), and a
hybrid one as `Negotiated TLS1.3 group`.

**D8. SOPS: policy refuses classical keys; the gate scans ciphertexts.**
(Done in slice 1.)
- `scripts/secrets.sh` refuses any key in `access.yaml` not starting
  `age1pq1`, naming the entry and the `age-keygen -pq` command.
- `scripts/secrets.sh check` (already `just secrets-check`, part of
  `just ci` and CI) also reads each committed `*.sops*` file's recipients
  and fails on any non-`age1pq1` recipient.
- Re-encryption alone doesn't help: the old ciphertext stays in git history.
  So every secret present before the cutover was rotated, which was only the
  Grafana admin password.

## Risks / Trade-offs

- [A trusted key leaks and is used to forge tokens] → Same blast radius as a
  leaked RSA private key today. Keys live only in SOPS (post-quantum since
  slice 1) and the backend's environment. Rotation (D5) revokes a leaked key
  within one deploy.
- [Algorithm confusion: an RS256 or `none` token accepted] → `Validation`
  allows only HS512, and tests cover RS256 signed with the old key, HS256
  under the right key, and `none`.
- [A retired key keeps working] → Verification looks keys up only in the
  configured map, with no fallback. Tests cover a retired key's session and
  reset tokens.
- [Old RS256 sessions are refused at cutover] → There's no production, so
  only local dev sessions are affected, and they refresh silently via the
  opaque refresh token.
- [Someone loses their only age key] → The key holder backs it up. The old
  classical key was kept until the re-encrypted files were verified to
  decrypt.

## Migration Plan

There are no schema changes. Order of slices, each releasable alone:

1. SOPS: PQ keys, policy refusal, the `secrets-check` scan and Grafana
   password rotation. Done 2026-11-12.
2. HMAC keyring: session, signup, reset and OAuth-state tokens switch to
   HS512 with `kid`, rotation via `JWT_TRUSTED_KEYS`, `just jwt-keygen`, and
   the RSA settings are removed.
3. Backend TLS: an explicit provider and the hybrid-only handshake test.
   Edge: the smoke check plus the Traefik contract in the deploy docs.

Rollback per slice is reverting the commit. For slice 2, that also means
restoring `JWT_PRIVATE_KEY`/`JWT_PUBLIC_KEY` in `backend/.env` (re-run
`./scripts/gen-dev-env.sh` from the reverted tree).

First-deploy notes (no production yet): generate the prod key with
`just jwt-keygen` straight into `just secrets-new`, give the prod host an
`age-keygen -pq` key, and run `just smoke https://<prod>` to confirm edge
key exchange.

## Open Questions

- Does Cloudflare proxy the public hostname in production? If it does,
  browser→Cloudflare is Cloudflare's TLS (already PQ), and D7's check applies
  to the Cloudflare→origin leg instead. The contract is the same either way,
  so this can wait until the first deploy.
