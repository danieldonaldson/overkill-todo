#!/bin/sh
# Render the app's public runtime config from the container's PUBLIC_* env into /tmp/config.js
# at container startup, so one promoted image serves every environment.
#
# Writes to /tmp because the container runs with readOnlyRootFilesystem (only the /tmp emptyDir
# is writable); nginx serves /config.js from /tmp (see nginx.conf). Runs via the nginx image's
# /docker-entrypoint.d hook (as uid 101, before nginx starts).
#
# Encoding approach (why not envsubst): `envsubst` would drop operator-set
# PUBLIC_* values straight into single-quoted JS string literals, with zero escaping. A value
# containing a single quote, backslash, or newline would break out of the string (or inject
# arbitrary JS). Instead we JSON-encode each value, assemble a JSON object, and emit
# `window.__PUBLIC_CONFIG = JSON.parse('<json>')`. Two escaping layers, kept consistent so
# values round-trip exactly:
#   1. json_string() -> valid JSON string (escapes \  "  tab  CR  and newline).
#   2. sed -> escapes \ and ' so the JSON survives as a single-quoted JS string literal, and
#      rewrites "</" to "<\/" as defense-in-depth (harmless: /config.js is served as an external
#      script, but this keeps a literal </script> in a value inert even if ever inlined).
# JSON.parse then reverses layer 1 at runtime. Only busybox awk + sed are needed (both in the
# alpine base); no jq/gettext dependency.
#
# The static build ships public/config.js with ${VAR} placeholders (served verbatim by the dev
# server, where config.ts falls back to import.meta.env). This entrypoint ignores those
# placeholders entirely and renders from env, so a broken/missing build artifact can't leak a
# literal ${VAR} into the served config.
set -eu

# Default so an unset env renders empty rather than an empty JSON error downstream.
: "${PUBLIC_TURNSTILE_SITE_KEY:=}"

src=/usr/share/nginx/html/config.js
dst=/tmp/config.js

# JSON-encode $1 (including the surrounding quotes) so arbitrary operator-set values are safe.
json_string() {
  printf '%s' "$1" | awk '
    BEGIN { ORS=""; sep=""; printf "\"" }
    {
      gsub(/\\/, "\\\\")
      gsub(/"/, "\\\"")
      gsub(/\t/, "\\t")
      gsub(/\r/, "\\r")
      printf "%s%s", sep, $0
      sep = "\\n"   # join input lines with an escaped newline (embedded newlines in the value)
    }
    END { printf "\"" }
  '
}

if [ ! -f "$src" ]; then
  # Not fatal — we render from env regardless — but a missing build artifact is worth flagging.
  echo "40-render-public-config: $src missing — rendering config from env only" >&2
fi

# Add a key here (and to PublicConfig in src/lib/config.ts) for each new PUBLIC_* value.
json=$(printf '{"TURNSTILE_SITE_KEY":%s}' "$(json_string "$PUBLIC_TURNSTILE_SITE_KEY")")

# Escape for the single-quoted JS string literal (layer 2). Backslash-doubling must run first so
# the "</" rewrite's own backslash is not re-doubled.
esc=$(printf '%s' "$json" | sed -e 's/\\/\\\\/g' -e "s/'/\\\\'/g" -e 's|</|<\\/|g')

printf 'window.__PUBLIC_CONFIG = JSON.parse('\''%s'\'');\n' "$esc" > "$dst"
