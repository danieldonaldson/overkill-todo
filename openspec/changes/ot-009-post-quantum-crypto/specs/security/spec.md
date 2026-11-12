# Spec Delta

## Purpose

Defines the cryptographic guarantees the system makes so that what it signs,
encrypts or sends today cannot be forged or decrypted later, including by an
attacker with a large quantum computer.

## ADDED Requirements

### Requirement: Issued tokens carry a post-quantum signature

Every token the backend issues and later checks itself (session, signup,
password-reset and OAuth-state tokens) SHALL be signed with ML-DSA-44
(FIPS 204). Each token SHALL name the algorithm and the key that signed it. The
backend SHALL accept a token only if it names ML-DSA-44, names a key the
backend currently trusts, carries a valid signature from that key, and passes
the existing expiry, issuer, audience and token-type checks. Anything else
SHALL be refused the same way an expired or missing token is today, without
revealing which check failed.

#### Scenario: Signing in issues a post-quantum session token

- **WHEN** a Member signs in with email and password or with Google
- **THEN** the session token they receive names ML-DSA-44 and the key that signed it
- **AND** their next authenticated request succeeds

#### Scenario: A classically signed token is refused

- **WHEN** a request carries a session token that is validly signed with RS256, or names no algorithm, `none`, or an HMAC algorithm
- **THEN** the backend responds 401 with the same error body as for a missing token

#### Scenario: A token naming an unknown key is refused

- **WHEN** a request carries a token naming a key the backend does not trust
- **THEN** the backend responds 401 and does not try other keys

#### Scenario: A tampered token is refused

- **WHEN** any byte of a token's header, claims or signature is changed
- **THEN** the backend responds 401

#### Scenario: A token of the wrong type is refused

- **WHEN** a valid signup or password-reset token is sent where a session token is expected, or the other way round
- **THEN** the backend refuses it, as it does today

#### Scenario: The largest session token fits in the session cookie

- **WHEN** a Super Admin (the longest role) with a verified email signs in
- **THEN** the session cookie, name and attributes included, is under 4096 bytes
- **AND** signing in and making authenticated requests work in Chromium, Firefox and WebKit, desktop and mobile emulation

#### Scenario: Signing in with Google still round-trips its state

- **WHEN** a Visitor starts Google sign-in and Google redirects back
- **THEN** the OAuth state the backend issued is accepted and the sign-in completes
- **AND** a state that was tampered with, expired, or signed by an untrusted key is refused

### Requirement: Signing keys rotate without signing anyone out

The backend SHALL sign new tokens with exactly one active key and SHALL accept
tokens from that key and from any other key listed as still trusted. Adding a
new active key SHALL NOT invalidate tokens signed by the previous one while it
stays trusted. Removing a key from the trusted set SHALL invalidate its tokens.
A Member whose session token stops being accepted SHALL be refreshed silently
when their refresh token is still valid.

#### Scenario: Tokens from the previous key keep working after rotation

- **WHEN** the operator makes a new key active and keeps the previous key trusted, and a Member's session token was signed by the previous key
- **THEN** the Member's requests keep succeeding
- **AND** the next token they are issued names the new key

#### Scenario: Retiring a key signs its sessions in again silently

- **WHEN** the operator removes a key from the trusted set and a Member's session token was signed by it
- **THEN** their next request gets 401, the dashboard refreshes the session, and the Member stays signed in without seeing a sign-in page

#### Scenario: Retiring a key invalidates its one-time tokens

- **WHEN** a key is removed from the trusted set while a signup or password-reset link signed by it is outstanding
- **THEN** that link is refused as expired and the Visitor can request a new one

#### Scenario: Misconfigured keys stop the backend from starting

- **WHEN** the backend starts with no active key, a key that is not a valid ML-DSA-44 key, a key with no name, or two keys with the same name
- **THEN** it refuses to start and names the problem without printing any key material

### Requirement: Committed secrets are encrypted only to post-quantum recipients

Every encrypted secret committed to the repository SHALL be encrypted only to
hybrid post-quantum (ML-KEM-768 + X25519) recipients. The secrets access policy
SHALL refuse classical recipients. Every secret that was ever committed
encrypted to a classical recipient SHALL be replaced with a new value.

#### Scenario: A classical key in the access policy is refused

- **WHEN** someone adds a person or machine with a classical age key to the access policy and syncs it
- **THEN** the sync fails, names the entry, and says how to generate a post-quantum key
- **AND** no secret is re-encrypted

#### Scenario: A secret encrypted to a classical recipient fails the gate

- **WHEN** any committed secret has a classical recipient, whether added by hand or by an old tool
- **THEN** the full gate fails and names the file

#### Scenario: People and machines can still read their secrets

- **WHEN** a person or machine cleared for an environment decrypts a secret in it with their post-quantum key
- **THEN** they get the plaintext, and someone not cleared for it cannot

#### Scenario: Secrets that were exposed to classical encryption are rotated

- **WHEN** the switch to post-quantum recipients is complete
- **THEN** every secret that existed before it has a new value, and the old value no longer grants access anywhere

### Requirement: Connections that cross a network use post-quantum key exchange

Every TLS connection the project controls that crosses a network (browser to
edge, and backend to external services) SHALL offer hybrid X25519MLKEM768 key
exchange as its first choice. It SHALL fall back to classical key exchange only
when the peer doesn't support the hybrid, so older clients and services keep
working.

#### Scenario: A modern browser gets post-quantum key exchange

- **WHEN** a current Chromium, Firefox or Safari connects to the site
- **THEN** the connection's key exchange is X25519MLKEM768

#### Scenario: An older client still connects

- **WHEN** a client that supports only classical key exchange connects to the site
- **THEN** it connects with classical key exchange and the site works

#### Scenario: Backend calls to external services offer the hybrid first

- **WHEN** the backend opens a TLS connection to an external service such as Google
- **THEN** its first offered key-exchange group is X25519MLKEM768
- **AND** if the service does not support it, the call still succeeds classically

#### Scenario: Losing the hybrid fails the gate

- **WHEN** a dependency or configuration change stops the backend or the edge from offering X25519MLKEM768 first
- **THEN** the full gate fails and says which side lost it
