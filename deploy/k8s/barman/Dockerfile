# Barman server for the local cluster's database (OT-014 D7a). Its Barman must be
# the barman-cloud plugin sidecar's (3.20.1 for v0.16.0): a newer one writes
# backup metadata the plugin can't read. BARMAN_VERSION in versions.env.
FROM debian:trixie-slim@sha256:8aa5a7bf5a1955bddbe7d74fcdd0b91ab30e74362a8b73af672b0481b35b44a0
ARG BARMAN_VERSION
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
 && install -d /usr/share/postgresql-common/pgdg \
 && curl -fsSo /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc https://www.postgresql.org/media/keys/ACCC4CF8.asc \
 && echo "deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] https://apt.postgresql.org/pub/repos/apt trixie-pgdg main" \
    > /etc/apt/sources.list.d/pgdg.list \
 && apt-get update && apt-get install -y --no-install-recommends \
    "barman=$BARMAN_VERSION" "barman-cli-cloud=$BARMAN_VERSION" "python3-barman=$BARMAN_VERSION" postgresql-client-18 \
 && apt-get purge -y curl && apt-get autoremove -y && rm -rf /var/lib/apt/lists/*
COPY run.sh /usr/local/bin/barman-run
USER barman
ENTRYPOINT ["/usr/local/bin/barman-run"]
