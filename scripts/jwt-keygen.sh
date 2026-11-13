#!/usr/bin/env bash
# Print a new token key for JWT_SIGNING_KEY / JWT_TRUSTED_KEYS:
# `<kid>:<64 random bytes, base64url>`. HMAC-SHA-512 keys must be at least 64
# bytes (OT-009). The kid defaults to today's date.
#
#   ./scripts/jwt-keygen.sh [kid]
#   just jwt-keygen [kid] | just secrets-new deploy/secrets/prod/jwt_signing_key.sops
set -euo pipefail

kid=${1:-$(date +%Y-%m-%d)}
if ! [[ $kid =~ ^[A-Za-z0-9_-]{1,16}$ ]]; then
  echo "jwt-keygen: kid must be 1-16 characters from A-Z, a-z, 0-9, '_' and '-' (got '$kid')" >&2
  exit 1
fi
printf '%s:%s\n' "$kid" "$(openssl rand -base64 64 | tr -d '\n=' | tr '+/' '-_')"
