#!/bin/sh
# Run the startup hooks in /docker-entrypoint.d (in name order), then exec the CMD so the
# server is PID 1 and receives SIGTERM directly.
set -eu
for hook in /docker-entrypoint.d/*.sh; do
  [ -e "$hook" ] || continue
  echo "docker-entrypoint: running $hook" >&2
  "$hook"
done
exec "$@"
