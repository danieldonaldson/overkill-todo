# ---- build ----
FROM node:26-alpine AS build
# Node 25+ no longer bundles corepack; install it so pnpm still comes from `packageManager`.
RUN npm install -g corepack@0.36.0 && corepack enable
WORKDIR /repo
COPY pnpm-workspace.yaml package.json pnpm-lock.yaml .npmrc ./
COPY apps/web/package.json apps/web/
COPY packages/design-tokens/package.json packages/design-tokens/
RUN pnpm install --filter @overkill-todo/web... --frozen-lockfile
COPY . .
RUN pnpm --filter @overkill-todo/web build

# ---- dev (local Tilt inner loop: astro dev + Vite HMR, source synced by live_update) ----
# NOT the final stage — a no-target `docker build` (CI/prod) still lands on the Caddy `serve`
# stage below. Runs as the built-in non-root `node` user (uid 1000) to satisfy `restricted`
# PodSecurity, same posture as the backend `dev` stage. Deps install into the layer; Tilt's
# live_update syncs src/ into the running pod and Vite HMR patches the browser with no rebuild.
FROM node:26-alpine AS dev
RUN npm install -g corepack@0.36.0 && corepack enable
WORKDIR /repo
RUN chown node:node /repo
USER node
COPY --chown=node:node pnpm-workspace.yaml package.json pnpm-lock.yaml .npmrc ./
COPY --chown=node:node apps/web/package.json apps/web/
COPY --chown=node:node packages/design-tokens/package.json packages/design-tokens/
RUN pnpm install --filter @overkill-todo/web... --frozen-lockfile
COPY --chown=node:node . .
EXPOSE 8080
# Invoke astro's binary directly rather than via `pnpm … exec`: pnpm 11's
# verify-deps-before-run sees the image's FILTERED install as incomplete (the other workspace
# projects lack node_modules) and triggers a full reinstall at container start — slow, needs
# npm-registry egress, and re-runs husky. Running the bin skips all pnpm run-time behaviour.
# WORKDIR = the app dir so astro finds astro.config.mjs; exec form makes astro PID 1 so it
# receives SIGTERM for a clean stop. --host binds all interfaces (kubelet probe + ingress);
# --port matches the Service/probe. HMR clientPort + allowedHosts come from astro.config.mjs,
# env-driven (HMR_CLIENT_PORT / DEV_ALLOWED_HOSTS set on the dev deployment).
WORKDIR /repo/apps/web
CMD ["node_modules/.bin/astro","dev","--host","0.0.0.0","--port","8080"]

# ---- caddy binary ----
# The official image ships /usr/bin/caddy with the cap_net_bind_service FILE capability.
# Under the pod's `capabilities: drop [ALL]` + `allowPrivilegeEscalation: false`, exec'ing a
# binary whose file caps exceed the bounding set fails with EPERM, so strip it: we listen
# on 8080 and need no capabilities at all. Only the binary is taken from this image.
FROM caddy:2.11.9-alpine@sha256:0bbb055bda6793c999bde78b1b5ce3e89f829fb031af2cf50056ea1246559fee AS caddy
RUN setcap -r /usr/bin/caddy

# ---- serve ----
# Caddy on plain Alpine: busybox sh/awk/sed are all the config-render hook needs. The
# package manager is removed so a compromised process can't pull tools into the container.
# Runs as uid 101 on :8080 (no root, no privileged port) — the k8s securityContext pins it.
FROM alpine:3.24@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf
RUN apk --no-cache add ca-certificates mailcap \
    && apk --purge del apk-tools \
    && rm -rf /etc/apk /lib/apk /var/cache/apk /usr/share/apk
COPY --from=caddy /usr/bin/caddy /usr/bin/caddy
COPY apps/web/Caddyfile /etc/caddy/Caddyfile
COPY --from=build /repo/apps/web/dist /srv
# Runtime public config: the entrypoint runs /docker-entrypoint.d/*.sh (as uid 101) before
# exec'ing Caddy, rendering PUBLIC_* env into /tmp/config.js (root FS is read-only).
COPY --chmod=0755 apps/web/docker-entrypoint.sh /docker-entrypoint.sh
COPY --chmod=0755 apps/web/docker-entrypoint.d/ /docker-entrypoint.d/
# Caddy wants writable data/config dirs even with admin, autosave and auto-HTTPS off.
ENV XDG_DATA_HOME=/tmp/caddy/data XDG_CONFIG_HOME=/tmp/caddy/config
USER 101:101
EXPOSE 8080
ENTRYPOINT ["/docker-entrypoint.sh"]
CMD ["caddy", "run", "--config", "/etc/caddy/Caddyfile", "--adapter", "caddyfile"]
