# OT-009: Post-quantum cryptography

## Why

As a Member, I want my session and my data in transit to stay safe even once
large quantum computers exist, so that nothing recorded or committed today can
be decrypted or forged later.

Quantum computers break public-key cryptography (RSA, elliptic curves,
X25519), not symmetric ciphers or hashes. Three places in the project rely on
public-key crypto:

- **Encrypted secrets committed to git** are wrapped with classical age
  (X25519) keys. Anyone with a copy of the repo keeps the ciphertext forever,
  so these can be harvested now and decrypted later. The at-rest encryption
  key, CSRF secret and token-signing key are all meant to live there, so this
  exposure protects everything else.
- **TLS** to browsers and to Google. Traffic recorded today could be
  decrypted later unless the key exchange is post-quantum.
- **Session, signup, reset and OAuth-state tokens** are signed with RS256. A
  quantum attacker could recover the signing key from the public key, forge
  tokens and sign in as anyone, including a Super Admin.

Now is the cheapest time: there is no production yet, so keys can be replaced
without migrating live sessions or secrets that users depend on.

AES-256-GCM, HMAC-SHA256, Argon2 and SHA-256 are already quantum-safe and stay
as they are.

## What Changes

- Committed secrets are encrypted only to hybrid post-quantum
  (ML-KEM-768 + X25519) recipients. The access policy refuses classical keys,
  and every secret ever encrypted to a classical key is rotated.
- Tokens the backend issues and checks are signed with HMAC-SHA-512 instead
  of RS256. The backend is the only party that ever verifies them, so a
  public-key signature buys nothing, and a symmetric MAC is quantum-safe for
  the same reason AES-256 is. Each token names its signing key, the backend
  can hold several keys at once, and keys can be rotated without signing
  anyone out. **BREAKING** for any outstanding token: tokens issued before the
  switch are rejected and Members sign in again (or are refreshed silently).
- Every TLS connection we control that crosses a network offers hybrid
  X25519MLKEM768 key exchange first and falls back to classical only for
  peers that can't do it. A check in the gate fails if an upgrade drops it.

## Capabilities

### New Capabilities
- `security`: the cryptographic guarantees the system makes: how tokens are
  signed and rotated, how committed secrets are encrypted, and how connections
  that cross a network exchange keys.

### Modified Capabilities
None.

## Appetite

Two weeks.

## Out of Scope

- Symmetric primitives and password hashing (already quantum-safe).
- Google's own signatures (its ID tokens are RS256; that's Google's to change)
  and Cloudflare/Turnstile's TLS.
- Links between containers on the same host (they don't cross a network). If
  one ever does, it falls under this capability.
- Post-quantum public-key signatures (ML-DSA) for our tokens. Considered
  and dropped on 2026-11-13: an ML-DSA-44 token adds about 3 KB to every
  request (about 2.4 Gbps of inbound traffic at 100k requests/s) and costs
  far more CPU to verify, for a benefit we only get if someone other than the
  backend verifies tokens. If that ever happens (for example, issuing OIDC
  tokens), it's a new change.
- Narrowing the session cookie to API paths. It's worth doing for request
  size, but it's not about cryptography.

## Dependencies

- age ≥ 1.3 and SOPS ≥ 3.13 on every machine that reads secrets. The
  installed tools support post-quantum `age1pq1…` keys. When this change
  was written every committed secret was encrypted to a classical key;
  slice 1 switched them over.
- The production edge (Traefik, not built yet) must use a TLS stack with the
  hybrid group enabled. Recorded as a first-deploy note.

## Ready

- **Riskiest unknown:** key handling. A symmetric key both signs and
  verifies, so every copy of a trusted key is as sensitive as the signing
  key, and rotation must never leave a window where Members are signed out or
  a retired key still works.
- **First slice:** SOPS with post-quantum keys (done 2026-11-12). Then every
  token signed with HMAC-SHA-512 under a named key, with rotation, in one
  slice.
