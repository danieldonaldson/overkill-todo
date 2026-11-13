# Tasks

Slice 1 (SOPS) was done first because it's small, independent of the rest,
and stops new secrets from ever being encrypted to a classical key.

## 1. Slice 1: committed secrets encrypted only to post-quantum keys

Delivers these scenarios from specs/security/spec.md (requirement "Committed
secrets are encrypted only to post-quantum recipients"): "A classical key in
the access policy is refused", "A secret encrypted to a classical recipient
fails the gate", "People and machines can still read their secrets", "Secrets
that were exposed to classical encryption are rotated".

- [x] 1.1 Generate a post-quantum age key (`age-keygen -pq`) for the prod-cleared person and put its `age1pq1…` public key in `deploy/secrets/access.yaml`; verify `just secrets-sync` regenerates `.sops.yaml` and re-keys every secret, reporting the classical key as removed
- [x] 1.2 Rotate every secret that existed before the switch (only `prod/grafana_admin_password.sops`) with a new value via `just secrets-new`; verify it decrypts with the post-quantum key and its value differs from the old one (compared by hash, never printed)
- [x] 1.3 Make `scripts/secrets.sh` accept only `age1pq1…` keys, refusing classical ones by name with the `age-keygen -pq` hint before anything is re-keyed; verify on a scratch copy that `sync` fails for a classical key and for a malformed key and leaves the secrets untouched
- [x] 1.4 Make `scripts/secrets.sh check` (`just secrets-check`, in `just ci` and CI) fail on any committed secret with a classical recipient, naming the file and recipient; verify on a scratch copy with a classical recipient added by hand
- [x] 1.5 Document post-quantum-only keys in `deploy/secrets/README.md` (`age-keygen -pq` everywhere a key is generated); verify no `age-keygen` without `-pq` remains in the docs
- [x] 1.6 Run `shellcheck scripts/secrets.sh` (needs a mise version for shellcheck) and verify `just ci` is green
- [x] 1.7 Commit `access.yaml`, `.sops.yaml`, the re-keyed secret, `scripts/secrets.sh` and the README together. First-deploy note: give each prod host an `age-keygen -pq` key under `machines:`

## 2. Slice 2: HMAC-SHA-512 tokens with key rotation

Delivers these scenarios from specs/security/spec.md:
- Issued tokens carry a quantum-safe signature: all six scenarios.
- Signing keys rotate without signing anyone out: all four scenarios.

- [x] 2.1 Add `TokenKeyring` to `infrastructure/crypto.rs` (design D2–D4): load `JWT_SIGNING_KEY` and an optional `JWT_TRUSTED_KEYS` in settings, give `CryptoInitError` one variant per misconfiguration, refuse to start if `JWT_PRIVATE_KEY`/`JWT_PUBLIC_KEY` are set, and add `sign`/`verify` (HS512 only, `kid` lookup with no fallback). Verify with unit tests for every "Misconfigured keys" case (asserting no key bytes in the messages) and every rejection: RS256 signed by the old key, HS256 under the right key, `none`, missing `kid`, unknown `kid`, and changes to header, claims and signature
- [x] 2.2 Switch session tokens (`api/jwt.rs`, `middleware/auth.rs`, `middleware/csrf.rs`, `handlers/session.rs`) to the keyring. Verify the existing jwt and auth tests pass with `just test`
- [x] 2.3 Switch signup and reset tokens (`services/jwt_service.rs`) to the keyring, keeping the `JwtService` port unchanged. Verify the jwt_service tests, including wrong-type refusal, pass with `just test`
- [x] 2.4 Switch OAuth state (`api/oauth_state.rs`, `handlers/oauth_handler.rs`) to the keyring, and give the Google ID-token tests their own RSA test key. Verify the OAuth unit tests and `tests/oauth_integration.rs` pass with `just test` and `just test-integration`
- [x] 2.5 Add rotation tests: a token signed by a key in `JWT_TRUSTED_KEYS` is accepted and the next token names the active key; a session or reset token from a key no longer trusted is refused; after a 401 from a retired key, refresh issues a new token. Verify with `just test-integration`
- [x] 2.6 Log the active and trusted `kid`s at startup, never secrets, and verify with a test that the log line holds the names and no key bytes
- [x] 2.7 Add `just jwt-keygen [kid]`, switch `scripts/gen-dev-env.sh`, `backend/.env.example`, `tests/common` and the env docs in `crypto.rs` to the new settings, and remove the RSA settings. Verify by regenerating `backend/.env` from scratch and checking the backend starts and a sign-in works locally
- [x] 2.8 Run `/code-review high` on the slice and fix the findings. Verify `just ci` is green
- [x] 2.9 Record what slice 2 showed in design.md Hypothesis and add slice 3 to this file

## 3. Slice 3: post-quantum key exchange on connections that cross a network

Delivers these scenarios from specs/security/spec.md (requirement
"Connections that cross a network use post-quantum key exchange"): "A modern
browser gets post-quantum key exchange", "An older client still connects",
"Backend calls to external services offer the hybrid first", "Losing the
hybrid fails the gate".

- [x] 3.1 In `main.rs`, install the aws-lc-rs provider as the process default with `kx_groups` starting at `X25519MLKEM768` (design D6), before any TLS client is built. Verify with a unit test that the installed default's first group is X25519MLKEM768
- [x] 3.2 Add `backend/tests/tls_key_exchange.rs`: an in-process rustls server that accepts only X25519MLKEM768, reached with the same reqwest client builder the Google and Turnstile adapters use; a second server accepting only X25519 proves the classical fallback. Verify both pass with `just test-integration`, and that the first fails if the provider's groups are set to X25519 only
- [x] 3.3 Add an edge key-exchange check to `just smoke <url>` (design D7): for an `https` URL, connect with `openssl s_client -groups X25519MLKEM768` and fail unless it's negotiated, and check that a classical-only client still connects. Pin OpenSSL ≥ 3.5 in `.mise.toml` if the system one is older. Verify it passes against a public host known to support the hybrid and fails against one that doesn't
- [x] 3.4 Document the edge contract (X25519MLKEM768 first, classical fallback; for Traefik, leave `curvePreferences` unset or list X25519MLKEM768 first) in `deploy/compose/README.md`, and add the first-deploy check. Verify `just ci` is green

## 4. Feature done

- [ ] 4.1 Verify every scenario in specs/security/spec.md is covered by a test or by a check in `just ci` / `just smoke`
- [ ] 4.2 Update README.md, backend/ARCHITECTURE.md and deploy/secrets/README.md for the new env vars, `just jwt-keygen`, key rotation (design D5), PQ age keys and the edge TLS contract. Verify by following the README setup from a clean checkout
- [ ] 4.3 Record the first-deploy notes from design.md Migration Plan where the existing first-deploy notes live, and verify they're listed there
- [ ] 4.4 Archive the change with `/opsx:archive` and verify `openspec/specs/security/spec.md` exists
