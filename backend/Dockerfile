# syntax=docker/dockerfile:1

# ── Build stage: static musl binary ─────────────────────────────────────────────
# Alpine's rust image targets x86_64-unknown-linux-musl natively, so a plain
# `cargo build --release` produces a fully static binary (no libc to copy).
FROM rust:1.96.0-alpine AS builder

# Build deps for the C-backed crates:
#   - aws-lc-rs (via rustls/jsonwebtoken): cmake + a C compiler + perl
#   - musl-dev / build-base: the musl C toolchain
#   - bash: some build scripts assume a non-busybox shell
#   - curl: utoipa-swagger-ui's build script downloads the Swagger UI bundle
#     at build time via the curl binary (build-stage only; not in the runtime image)
RUN apk add --no-cache musl-dev build-base cmake make perl bash pkgconfig ca-certificates curl

# Build against the pinned channel directly. RUSTUP_TOOLCHAIN takes precedence over
# rust-toolchain.toml, so rustup skips syncing that file's dev-only components
# (rustfmt/clippy) on first invocation — the image build needs neither, and the sync
# was pure latency (visible as "downloading component clippy" mid-build).
ENV RUSTUP_TOOLCHAIN=1.96.0

WORKDIR /app

# Cache dependency compilation separately from source.
COPY Cargo.toml Cargo.lock ./
# Dummy src so cargo can resolve and compile deps. The COMPILED deps land in the
# /app/target image layer (deliberately NOT a cache mount) so CI's registry buildcache
# (ci.yml cache-from/to) can restore them between runs. The cache mounts below
# only persist crate DOWNLOADS across local rebuilds — they sit outside /app, so they
# don't shadow that layer and can't regress the CI cache path.
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git \
    cargo build --release --locked
RUN rm -rf src

# Build the real binary. Force sqlx offline mode so the `query!` macros resolve
# against the committed `.sqlx/` cache instead of needing a live DATABASE_URL.
ENV SQLX_OFFLINE=true
COPY . .
# Touch to bust the dep-cache timestamp.
RUN touch src/main.rs src/lib.rs
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git \
    cargo build --release --locked

# ── Dev stage: fast incremental inner loop for Tilt (NOT built by CI) ────────────
# CI/`just build-backend` build the default (last) stage, so this one only runs when a
# builder explicitly targets it. Tilt does, if you add a Tiltfile, driving it with
# live_update: source is synced into the RUNNING container and cargo-watch recompiles
# in place, so /app/target stays warm across edits instead of a from-scratch musl
# release build + rollout per change. glibc + lld link far faster than the static-musl
# release link, and this image never ships to prod.
#
# It still runs non-root (uid 10001) so it passes a `restricted` PodSecurity namespace;
# a dev overlay only needs to relax readOnlyRootFilesystem (so cargo can write
# /app/target) and the probes/resources.
FROM rust:1.96.0-bookworm AS dev

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential cmake perl pkg-config clang lld libssl-dev bash curl \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-watch --locked

# Same rationale as the builder stage: skip the dev-only component sync.
ENV RUSTUP_TOOLCHAIN=1.96.0
ENV SQLX_OFFLINE=true
# lld is the single biggest win on debug-rebuild wall-clock.
ENV RUSTFLAGS="-C link-arg=-fuse-ld=lld"

WORKDIR /app

# Pre-compile deps into the image LAYER, and deliberately NO cargo cache mounts here
# (unlike the builder stage). A build-time cache mount keeps the crate index + downloaded
# sources OUT of the final image, so the runtime `cargo run` below would have to reach
# crates.io to resolve — which an egress NetworkPolicy may block, leaving cargo
# stuck in SYN_SENT and the server never binding :4322. Baking the registry into the layer
# lets cargo resolve fully offline against the warm deps; only our crate compiles at runtime.
COPY Cargo.toml Cargo.lock ./
# .cargo/config.toml MUST land before the dep build, not with the `COPY . .` below. It sets
# `linker = "clang"` for x86_64-unknown-linux-gnu — exactly this stage's target — and the
# linker is part of every unit's fingerprint. Compiling deps without it, then letting the
# in-pod `cargo watch` see it for the first time, invalidates all ~477 crates and recompiles
# the whole graph on EVERY fresh pod (the baked target/ is warm for the wrong linker).
# (RUSTFLAGS below is not the issue: the env var overrides the config's identical rustflags.)
COPY .cargo ./.cargo
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs
# Grant the runtime user (uid 10001) read access to the baked registry: cargo extracts
# crate sources here as ROOT, and some crates ship their files 0640 in the .crate tarball
# (fnv is one), which cargo preserves — so the 10001 in-pod `cargo watch` recompile would
# otherwise hit "Permission denied" reading e.g. fnv-1.0.7/lib.rs. `a+rX` adds read to files
# and traverse to dirs without touching /app/target; folded into this RUN so it mutates the
# just-created files in place (no extra layer / copy-up bloat).
RUN cargo build --locked \
    && chmod -R a+rX "$CARGO_HOME/registry"
RUN rm -rf src

# From here cargo must never hit the network: deps + index are baked above, and the egress
# policy would otherwise hang the resolve. This is what keeps the dev pod coming up reliably.
ENV CARGO_NET_OFFLINE=true

COPY . .
# Two things happen here, and both are load-bearing:
#
#   touch — the dummy-src build left a placeholder `app` binary in target/, and COPY
#   stamps the real sources with their (older) build-context mtimes. Cargo compares mtimes,
#   so it would call the crate FRESH and `cargo watch -x run` would exec the no-op dummy,
#   which exits 0 immediately. Same trick as the builder stage.
#
#   cargo build — compile the REAL binary into the image layer. Only our crate builds: the
#   dep graph is already fresh, now that .cargo/config.toml was present when it was compiled.
#   This is what makes a cold pod (scale-to-zero, rollout, eviction, node reboot) start warm
#   instead of recompiling. The pod's writable layer is discarded on every restart, so
#   anything not baked here is paid for again on each fresh pod.
RUN touch src/main.rs src/lib.rs && cargo build --locked
# cargo (uid 10001) owns /app so it can write target/ under `restricted` PodSecurity.
# The official rust image already makes CARGO_HOME/RUSTUP_HOME world-writable.
RUN chown -R 10001:10001 /app
USER 10001:10001

EXPOSE 4322
# cargo-watch recompiles + reruns the debug binary on every synced source change.
CMD ["cargo", "watch", "-x", "run"]

# ── Runtime image: scratch ──────────────────────────────────────────────────────
# Nothing but the static binary and CA certs. No shell, no package manager, no
# libc — minimal attack surface and a tiny image.
FROM scratch

# TLS trust store for outbound HTTPS (OAuth, SMTP, third-party APIs).
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

COPY --from=builder /app/target/release/app /usr/local/bin/app

# Migrations are embedded via the sqlx migrate! macro; no runtime file copy needed.

# Run as a non-root, unprivileged UID. scratch has no /etc/passwd, so reference
# the user numerically; the server binds 4322 (>1024) and writes nothing to disk.
USER 10001:10001

EXPOSE 4322

# scratch has no curl/shell, so the binary probes itself via its `healthcheck`
# subcommand (a dependency-free GET /health on the loopback port). /health is the
# liveness probe; the DB-backed readiness probe is /health/ready.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["app", "healthcheck"]

ENTRYPOINT ["app"]
