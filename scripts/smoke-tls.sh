#!/usr/bin/env bash
# Read-only check of a deployed edge's TLS key exchange (OT-009 design D7):
# it must negotiate hybrid post-quantum X25519MLKEM768 with a client that
# offers it, and still accept a classical-only client (X25519).
# Skips plain-http URLs (local dev has no edge TLS).
#
#   ./scripts/smoke-tls.sh https://app.example.com
set -euo pipefail

url=${1:?usage: smoke-tls.sh <url>}
if [[ $url != https://* ]]; then
  echo "smoke-tls: $url is not https; skipping the edge key-exchange check."
  exit 0
fi
authority=${url#https://}
authority=${authority%%/*}
host=${authority%%:*}
port=443
[[ $authority == *:* ]] && port=${authority##*:}

# One handshake offering only the given group; prints the negotiated group, or
# nothing if the handshake failed. OpenSSL reports a hybrid group as
# "Negotiated TLS1.3 group" and a classical one as "Peer Temp Key" (3.5+) or
# "Server Temp Key" (older).
negotiate() {
  local out
  out=$(timeout 15 openssl s_client -connect "$host:$port" -servername "$host" -groups "$1" </dev/null 2>&1 || true)
  sed -n 's/^Negotiated TLS1.3 group: //p; s/^\(Peer\|Server\) Temp Key: \([^,]*\).*/\2/p' <<<"$out" | head -1
}

# An openssl that doesn't know the group fails while parsing -groups, before
# connecting; port 9 (discard) is never reached either way.
probe=$(timeout 5 openssl s_client -groups X25519MLKEM768 -connect 127.0.0.1:9 </dev/null 2>&1 || true)
if grep -q 'SSL_CONF_cmd' <<<"$probe"; then
  echo "smoke-tls: this openssl ($(openssl version)) can't offer X25519MLKEM768; OpenSSL 3.5+ is needed." >&2
  exit 1
fi

errors=0
got=$(negotiate X25519MLKEM768)
if [[ $got == X25519MLKEM768 ]]; then
  echo "smoke-tls: $host:$port negotiates X25519MLKEM768 (post-quantum)."
else
  echo "smoke-tls: $host:$port did not negotiate X25519MLKEM768 with a client offering only it (got: ${got:-handshake failed})." >&2
  echo "  The edge must offer it first; for Traefik leave curvePreferences unset or list X25519MLKEM768 first." >&2
  errors=1
fi

got=$(negotiate X25519)
if [[ $got == X25519 ]]; then
  echo "smoke-tls: $host:$port still accepts classical-only clients (X25519)."
else
  echo "smoke-tls: $host:$port refused a classical-only client (got: ${got:-handshake failed}); older clients must still connect." >&2
  errors=1
fi
exit "$errors"
