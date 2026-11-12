# Tasks

Slice 1 (SOPS) was done first because it's small, independent of the rest,
and stops new secrets from ever being encrypted to a classical key. Slice 2
retires the riskiest unknown. Slices 3–4 (OAuth state and rotation, TLS; see
design.md Migration Plan) are added once slice 2 has shipped and design.md
records what it showed.

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

## 2. Slice 2: ML-DSA-44 session, signup and reset tokens

Delivers these scenarios from specs/security/spec.md:
- Issued tokens carry a post-quantum signature: "Signing in issues a
  post-quantum session token", "A classically signed token is refused", "A
  token naming an unknown key is refused", "A tampered token is refused", "A
  token of the wrong type is refused", "The largest session token fits in the
  session cookie".
- Signing keys rotate without signing anyone out: "Misconfigured keys stop the
  backend from starting".

- [ ] 2.1 Add `aws-lc-rs` as a direct backend dependency (infrastructure only) and verify `just machete` and `cargo tree -d` show no duplicate aws-lc-rs versions
- [ ] 2.2 Add `infrastructure/crypto/token.rs`: compact JWS sign and verify for `ML-DSA-44` per design D3 (size cap, strict header, kid lookup, verify before parsing claims). Verify with unit tests for the round trip and every rejection branch (RS256 token from the old key, `alg: none`, HS256, unknown kid, extra header fields, wrong segment count, padding, bit-flips in each segment, oversized input) plus a proptest that random input never panics or verifies
- [ ] 2.3 Add a known-answer test from the JOSE ML-DSA draft's ML-DSA-44 example if one is published; otherwise record in design.md D3 that none exists, and verify with `just test`
- [ ] 2.4 Replace the RSA keys in `Crypto` with a `TokenKeyring` loaded from `JWT_SIGNING_KEY` and an optional `JWT_TRUSTED_KEYS` (design D4/D5). Add `CryptoInitError` variants for each misconfiguration and refuse to start if `JWT_PRIVATE_KEY`/`JWT_PUBLIC_KEY` are set. Verify with unit tests for each "Misconfigured keys" case, asserting no key bytes appear in the messages
- [ ] 2.5 Switch session tokens (`infrastructure/api/jwt.rs`, `middleware/auth.rs`, `handlers/session.rs`) and `CryptoJwtService` (signup and reset) to the keyring, keeping the `JwtService` port unchanged. Verify the existing jwt, jwt_service and auth-handler tests pass with `just test` and `just test-integration`
- [ ] 2.6 Add a cookie-budget unit test: the longest possible claims (super_admin, verified, a 16-character kid) produce an `auth_token` `Set-Cookie` under 4,000 bytes. Verify with `just test`
- [ ] 2.7 Add `backend jwt-keygen [kid]` next to `healthcheck` in `main.rs`, a `just jwt-keygen` recipe, and switch `scripts/gen-dev-env.sh` to it. Update `backend/.env.example` and the env-var docs in `crypto.rs`. Verify by running `./scripts/gen-dev-env.sh` on a clean `backend/.env` and starting `just dev`
- [ ] 2.8 Add Firefox and WebKit (desktop, plus one mobile-emulation device each for Chromium and WebKit) to `playwright.integration.config.ts` and install their browsers in `just setup`. Add a real-backend flow in `apps/web/tests/integration/` that signs in, loads the dashboard and makes an authenticated call. Verify `just e2e-integration` passes on every project
- [ ] 2.9 Run the `auth-mix` load scenario and verify it stays within the performance limits (`just load auth-mix`)
- [ ] 2.10 Log startup key configuration as `kid`s only (active and trusted), never key material, and verify with a test that captures the startup log
- [ ] 2.11 Run `/code-review high` on the token module and fix the findings. Verify `just ci` is green
- [ ] 2.12 Record what slice 2 showed (cookie size per browser, load numbers) in design.md Hypothesis, revise decisions it changed, and add slices 3–4 to this file

## 3. Feature done

- [ ] 3.1 Verify every scenario in specs/security/spec.md is covered by a test or by a check in `just ci` / `just smoke`
- [ ] 3.2 Update README.md, backend/ARCHITECTURE.md and deploy/secrets/README.md for the new env vars, `just jwt-keygen`, PQ age keys and the edge TLS contract. Verify by following the README setup from a clean checkout
- [ ] 3.3 Record the first-deploy notes from design.md Migration Plan where the existing first-deploy notes live, and verify they're listed there
- [ ] 3.4 Archive the change with `/opsx:archive` and verify `openspec/specs/security/spec.md` exists
