# ---- build ----
FROM node:24-alpine AS build
RUN corepack enable
WORKDIR /repo
COPY pnpm-workspace.yaml package.json pnpm-lock.yaml .npmrc ./
COPY apps/web/package.json apps/web/
COPY packages/design-tokens/package.json packages/design-tokens/
RUN pnpm install --filter @app/web... --frozen-lockfile
COPY . .
RUN pnpm --filter @app/web build

# ---- dev (local Tilt inner loop: astro dev + Vite HMR, source synced by live_update) ----
# NOT the final stage — a no-target `docker build` (CI/prod) still lands on the nginx `serve`
# stage below. Runs as the built-in non-root `node` user (uid 1000) to satisfy `restricted`
# PodSecurity, same posture as the backend `dev` stage. Deps install into the layer; Tilt's
# live_update syncs src/ into the running pod and Vite HMR patches the browser with no rebuild.
FROM node:24-alpine AS dev
RUN corepack enable
WORKDIR /repo
RUN chown node:node /repo
USER node
COPY --chown=node:node pnpm-workspace.yaml package.json pnpm-lock.yaml .npmrc ./
COPY --chown=node:node apps/web/package.json apps/web/
COPY --chown=node:node packages/design-tokens/package.json packages/design-tokens/
RUN pnpm install --filter @app/web... --frozen-lockfile
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

# ---- serve ----
# Non-root, version-pinned nginx: nginx-unprivileged runs as uid 101 and listens
# on 8080 (nginx.conf reflects this); no root, no privileged port.
FROM nginxinc/nginx-unprivileged:1.30-alpine
COPY apps/web/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /repo/apps/web/dist /usr/share/nginx/html
# Runtime public config: the nginx image runs /docker-entrypoint.d/*.sh (as uid 101) before
# starting, rendering PUBLIC_* env into /tmp/config.js (root FS is read-only). --chmod needs
# BuildKit, which both CI (buildx) and local docker use by default.
COPY --chmod=0755 apps/web/docker-entrypoint.d/40-render-public-config.sh /docker-entrypoint.d/40-render-public-config.sh
EXPOSE 8080
CMD ["nginx", "-g", "daemon off;"]
