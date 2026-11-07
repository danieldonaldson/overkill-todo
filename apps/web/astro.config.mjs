import { defineConfig } from 'astro/config'
import { fileURLToPath } from 'node:url'
import svelte from '@astrojs/svelte'
import tailwindcss from '@tailwindcss/vite'

// Static SSG app. All auth/dashboard behaviour lives in client-side Svelte islands
// that call the Rust API at the same origin (/api/v1). In `just dev` Vite proxies
// /api to the backend; deployed, the edge (Caddy or an Ingress) routes it.

// Where the dev/preview server forwards /api. The backend's default port.
const apiTarget = process.env.API_PROXY_TARGET ?? 'http://localhost:4322'

// Behind a TLS proxy (e.g. the optional k8s dev loop), the HMR websocket must dial
// back on the proxy port (HMR_CLIENT_PORT=443), not Vite's own port.
const hmrClientPort = process.env.HMR_CLIENT_PORT
  ? Number(process.env.HMR_CLIENT_PORT)
  : undefined

// Vite rejects requests whose Host header isn't localhost unless allow-listed. Behind a
// proxy with its own hostname the dev server 403s ("Blocked request") without this.
// Comma-separated, env-driven (DEV_ALLOWED_HOSTS) so no dev hostname is baked into config.
const devAllowedHosts = process.env.DEV_ALLOWED_HOSTS
  ? process.env.DEV_ALLOWED_HOSTS.split(',')
  : undefined

export default defineConfig({
  output: 'static',
  // No landing page: `/` goes to sign-in. Put marketing in its own site (shared
  // design tokens only). Under `output: 'static'` this emits a meta-refresh index.html.
  redirects: { '/': '/login' },
  integrations: [svelte()],
  server: { port: 4321 },
  // The Astro dev toolbar wedges behind single-origin proxies (its entrypoint.js
  // request 504s). It's a dev-only overlay we don't use, so disable it.
  devToolbar: { enabled: false },
  vite: {
    plugins: [tailwindcss()],
    resolve: {
      // mode-watcher holds the theme in a module-level store. Astro loads the
      // <ModeWatcher> island from the raw package file while ThemeToggle's bare
      // import resolves to Vite's pre-bundled copy — two instances, two stores,
      // so toggleMode() never reaches the watcher that sets html.dark. Excluding
      // it from pre-bundling (plus dedupe) forces a single shared instance.
      dedupe: ['mode-watcher'],
      alias: {
        // Mirror SvelteKit's $lib so ported shadcn-svelte components resolve.
        $lib: fileURLToPath(new URL('./src/lib', import.meta.url)),
        '@': fileURLToPath(new URL('./src', import.meta.url)),
      },
    },
    optimizeDeps: {
      exclude: ['mode-watcher'],
    },
    server: {
      proxy: { '/api': { target: apiTarget, changeOrigin: false } },
      // Vite 8 moved the HMR client-connection options from `server.hmr.*` to `server.ws.*`.
      ...(hmrClientPort ? { ws: { clientPort: hmrClientPort } } : {}),
      ...(devAllowedHosts ? { allowedHosts: devAllowedHosts } : {}),
    },
    preview: {
      proxy: { '/api': { target: apiTarget, changeOrigin: false } },
    },
  },
})
