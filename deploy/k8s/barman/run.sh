#!/bin/sh
# What cron does on a Barman host: `barman cron` (streaming, WAL processing, local
# retention) every 30 s, a backup daily at 03:00 UTC, the first as soon as Barman
# can take one, and the same retention in Garage. Upload to Garage is the hooks'
# job (barman.conf).
set -u
server=${BARMAN_SERVER:?}
cloud="--endpoint-url $GARAGE_URL --addressing-style path"
done_day=
while true; do
  barman -q cron
  if [ -z "$(barman list-backups --minimal "$server")" ]; then
    if barman check "$server" 2>/dev/null | grep -q 'receive-wal running: OK'; then
      barman backup --wait "$server" && done_day=$(date -u +%F)
    fi
  elif [ "$(date -u +%H)" = 03 ] && [ "$done_day" != "$(date -u +%F)" ]; then
    # shellcheck disable=SC2086
    barman backup --wait "$server" &&
      barman-cloud-backup-delete $cloud --retention-policy "$RETENTION" "$GARAGE_DEST" "$server" &&
      done_day=$(date -u +%F)
  fi
  sleep 30
done
