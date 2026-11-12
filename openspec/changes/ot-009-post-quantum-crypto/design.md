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
  RS256 JWKS (`infrastructure/oauth/google.rs`). That stays as it is.
- **TLS.** rustls 0.23.46 with the aws-lc-rs provider and its default
  `prefer-post-quantum` feature already offers X25519MLKEM768 first. reqwest
  (Google, Turnstile) uses rustls. The connections to Postgres, the OTel
  collector and Prometheus are plaintext on the single-host Docker network
  (out of scope). The local Caddy runs `auto_https off`, so there's no local
  edge TLS. The production edge (Traefik) isn't built yet.
- **Secrets.** `deploy/secrets/access.yaml` holds one person's classical
  `age1…` key. `scripts/secrets.sh` generates `.sops.yaml` from it. The only
  committed secret today is `prod/grafana_admin_password.sops`; backend
  secrets aren't in SOPS yet. Installed versions: age 1.3.1, SOPS 3.13.2.
  `age-keygen -pq` plus a SOPS encrypt/decrypt round trip was checked to work
  on 2026-11-12.

## Goals / Non-Goals

**Goals:**
- One signing path in the backend for every token we issue, with no RS256
  left.
- Crypto agility: changing algorithm or key is configuration plus a small
  code change, never a rewrite.
- Each guarantee is enforced by the gate, not by memory.

**Non-Goals:**
- A general-purpose JOSE library. We implement exactly one algorithm, one
  serialisation (compact JWS) and only the header fields we use.
- Server-side sessions. Tokens stay stateless so the dashboard's optimistic
  path doesn't gain a database lookup.

## Hypothesis

We believe a ~3.5 KB ML-DSA-44 session token works unchanged through the
cookie, the edge and the backend in all supported browsers, and that a small
strict JWS module is safer than adapting a general library. Slice 2 tests
this end to end: a real-backend Playwright flow (`tests/integration/`) signs
in on Chromium, Firefox and WebKit (desktop and mobile emulation; today the
Playwright configs run Chromium only, so slice 2 adds the others), and a unit
test asserts the longest possible cookie is under 4096 bytes. If it fails, we
switch the session token's transport (D6 alternatives) before continuing.
There's no production yet, so the flag-free cutover in slice 2 is safe.

## Decisions

**D1. ML-DSA-44, pure (not pre-hash), hedged signing.** Level 2 is the
largest level whose tokens fit a cookie (ML-DSA-65's 3,309-byte signature
makes the token ~4.7 KB, over the limit). The user chose it on 2026-11-12
over HS512 (symmetric, smaller, but gives up public-key verification) and
ML-DSA-65 (needs a new token transport). Alternatives considered: a composite
of ML-DSA and Ed25519 (least standardised in JOSE, little benefit given a
15-minute token lifetime).

**D2. Implementation: `aws-lc-rs` directly, in infrastructure.** It's already
in the dependency tree via rustls and `jsonwebtoken`, it's FIPS-validated
upstream, and ML-DSA has been stable in `aws_lc_rs::signature` since 1.18
(`PqdsaKeyPair::from_seed`, `UnparsedPublicKey` with `ML_DSA_44`).
Alternatives: RustCrypto `ml-dsa` (pre-1.0, unaudited), liboqs (another C
toolchain dependency), and `jsonwebtoken` (no ML-DSA support).

**D3. Token format: compact JWS, `alg` = `ML-DSA-44`.** This follows the
IETF JOSE ML-DSA draft (draft-ietf-cose-dilithium) so the tokens stay
readable by standard tooling later. The header is exactly
`{"alg":"ML-DSA-44","kid":"<kid>","typ":"JWT"}`. Verification order, all in
one module (`infrastructure/crypto/token.rs`):
1. Reject tokens over 8 KiB, and tokens that aren't exactly three base64url
   (no padding) segments.
2. Parse the header strictly: unknown fields are rejected, so `crit`, `jku`,
   `jwk` and `x5u` can never be honoured. `alg` must equal `ML-DSA-44`
   exactly.
3. Look up `kid` in the keyring. An unknown key fails, and no other keys are
   tried.
4. Verify the signature over the ASCII signing input, *before* the claims are
   parsed.
5. Parse the claims and check `exp` (60 s leeway, as `jsonwebtoken` uses
   today), `iss`, `aud` and `typ` as now.

Every failure maps to the existing `AppError::token_invalid` / `DomainError`
values, so the 401 body is unchanged. Tests: round trip, every rejection
branch, an RS256 token signed by the old key, `alg: none`, header/claims/
signature bit-flips, a proptest that random input never panics or verifies,
and the draft's ML-DSA-44 JOSE example as a known-answer test if the draft
publishes one. Alternatives: COSE/CWT (binary and smaller, but nothing in our
stack speaks it), PASETO (no PQ version), opaque server-side sessions (see
Non-Goals).

**D4. One `TokenKeyring` on `Crypto`; the ports are unchanged.** `Crypto`
replaces its RSA `EncodingKey`/`DecodingKey` with a keyring: one active
signing key and a `kid → public key` map. `jwt.rs`, `oauth_state.rs` and
`CryptoJwtService` all call it. The application-layer `JwtService` port and
the domain don't change. `jsonwebtoken` stays only for Google ID tokens.

**D5. Key configuration and generation.**
- `JWT_SIGNING_KEY=<kid>:<base64url 32-byte seed>` is the active key. Its
  public key is derived and implicitly trusted.
- `JWT_TRUSTED_KEYS=<kid>:<base64url 1312-byte public key>,…` is optional:
  previous keys that are still accepted.
- `JWT_PRIVATE_KEY` / `JWT_PUBLIC_KEY` are removed. Startup fails if they're
  still set, so stale config is noticed.
- A `kid` is 1–16 characters from `[A-Za-z0-9_-]`, e.g. `2026-11a`.
- `backend jwt-keygen [kid]` (next to the existing `healthcheck` argument)
  prints both lines, and `just jwt-keygen` wraps it. `scripts/gen-dev-env.sh`
  uses it instead of `openssl genpkey`.
- `CryptoInitError` gains variants for each startup failure in the spec.
  Messages name the variable and the `kid`, never key bytes.

**D6. Cookie and header budget.** Measured shape: header ≈ 80 chars,
longest claims ≈ 280, signature 3,227 (2,420 bytes in base64url), plus
`auth_token=` and attributes ≈ 70, for ≈ 3,660 bytes against the 4,096 limit.
A unit test builds the longest possible claims (super_admin, verified, a
maximum-length `kid`) and asserts the `Set-Cookie` value is under 4,000
bytes. Request headers stay well under hyper's and Cloudflare's limits. If
browsers reject it, the fallback is to move the session token into its own
cookie scheme (two halves) or a response body plus `Authorization` header.
Both would be a spec change, decided after slice 2.

**D7. OAuth state moves out of the URL.** A 3.5 KB `state` query parameter
to Google risks URL limits we can't see. Instead, `state` becomes the bare
random nonce. The signed claims (`provider`, `mode`, `user_id`, `nonce`,
`exp`) go in the existing HttpOnly, path-scoped `oauth_state` cookie as an
ML-DSA token. The callback verifies the cookie token and checks that its
`nonce` equals the `state` parameter. This keeps the existing guarantees
(browser binding, 5-minute expiry, tamper-evidence). Alternative: keeping the
signed state in the URL, rejected for the size risk.

**D8. Backend TLS: make the default explicit and test it.** At startup,
`main.rs` installs the aws-lc-rs provider as the process default with
`kx_groups` starting at `X25519MLKEM768`, rather than relying on a Cargo
feature default. A test starts an in-process rustls server that accepts
*only* X25519MLKEM768 and connects with the same reqwest client builder the
adapters use. If the hybrid is ever dropped, the handshake fails. The test
lives in `backend/tests/`, so it runs in `just test`.

**D9. Edge TLS: a proxy-agnostic contract.** The contract: whatever
terminates public TLS offers X25519MLKEM768 first and allows classical
fallback. For Traefik (Go ≥ 1.24), that means not setting
`curvePreferences`, or listing `X25519MLKEM768` first. The check is a new
read-only smoke test in `just smoke <url>`: for an `https` URL, connect with
`openssl s_client -groups X25519MLKEM768` (OpenSSL ≥ 3.5, pinned in
`.mise.toml` if needed) and fail unless it's negotiated. Locally there's no
edge TLS, so this check runs only against deployed URLs, which is a
first-deploy note.

**D10. SOPS: policy refuses classical keys; the gate scans ciphertexts.**
- `scripts/secrets.sh` refuses any key in `access.yaml` not starting
  `age1pq1`, naming the entry and the `age-keygen -pq` command.
- `scripts/secrets.sh check` (already `just secrets-check`, part of
  `just ci`) also reads each committed `*.sops*` file's
  `sops.age[].recipient` list and fails on any non-`age1pq1` recipient.
- Cutover: generate PQ keys, put them in `access.yaml`, run
  `just secrets-sync` (re-encrypts), then generate a new Grafana admin
  password with `just secrets-new` and delete the old private key.
- Re-encryption alone doesn't help: the old ciphertext stays in git history.
  So every secret present before the cutover is rotated, which today means
  only the Grafana admin password.

## Risks / Trade-offs

- [Hand-written token verification has a bug that accepts a bad token] →
  The module is small, strict and allow-list only. It has rejection tests for
  each branch, a proptest, and review with `/code-review high` before merge.
- [ML-DSA-44 is NIST level 2, below CNSA 2.0's level 5] → Accepted: the
  15-minute lifetime limits exposure, and D5's keyring makes moving up a key
  change once transport allows larger tokens.
- [Bigger tokens on every request (≈3.6 KB vs ≈0.6 KB)] → A few KB per
  request, which is negligible next to page assets. ML-DSA verification is
  tens of microseconds; the `auth-mix` load scenario confirms it's within the
  performance budgets.
- [The JOSE ML-DSA draft changes the `alg` name or key encoding] → We only
  talk to ourselves. Renaming is a one-line change plus a key rotation.
- [Old RS256 sessions are refused at cutover] → There's no production, so
  only local dev sessions are affected, and they refresh silently via the
  opaque refresh token.
- [The Google `state` redesign breaks the account-linking flow] → The
  existing OAuth handler tests cover link, sign-in and signup modes, and are
  extended for cookie/state mismatches.
- [Someone loses their only age key after cutover] → Same as today: the key
  holder backs up `~/.config/sops/age/keys.txt`. The old classical key is
  kept until the re-encrypted files are verified to decrypt.

## Migration Plan

There are no schema changes. Order of slices, each releasable alone:

1. SOPS: PQ keys, policy refusal, the `secrets-check` scan and Grafana
   password rotation. Done first (2026-11-12) because it's small,
   independent, and stops new secrets from ever being encrypted to a
   classical key.
2. ML-DSA keyring and token module. Session, signup and reset tokens switch
   over, RS256 is removed for them, and keygen plus dev-env generation are
   updated.
3. OAuth state moves to the cookie and is ML-DSA signed. Rotation via
   `JWT_TRUSTED_KEYS` gets its scenarios and tests. `jsonwebtoken` is now
   used only for Google ID tokens.
4. Backend TLS: an explicit provider and the hybrid-only handshake test.
   Edge: the smoke check plus the Traefik contract in the deploy docs.

Rollback per slice is reverting the commit. For 2–3, that also means
restoring `JWT_PRIVATE_KEY`/`JWT_PUBLIC_KEY` in `backend/.env` (re-run
`./scripts/gen-dev-env.sh`).

First-deploy notes (no production yet): generate the prod key with
`just jwt-keygen` straight into `just secrets-new`, give the prod host an
`age-keygen -pq` key, and run `just smoke https://<prod>` to confirm edge
key exchange.

## Open Questions

- Does Cloudflare proxy the public hostname in production? If it does,
  browser→Cloudflare is Cloudflare's TLS (already PQ), and D9's check applies
  to the Cloudflare→origin leg instead. The contract is the same either way,
  so this can wait until the first deploy.
