// Local preview through the same Caddyfile the web image runs (design D8):
// `astro preview` intentionally drops user Vite plugins, so it cannot serve
// the one shell for every list address. Playwright therefore runs against the
// web server configuration, rooted at the built site.
import { spawn } from 'node:child_process'

const args = process.argv.slice(2)
const portIndex = args.indexOf('--port')
const port = portIndex === -1 ? '4323' : (args[portIndex + 1] ?? '4323')

const api = process.env.API_PROXY_TARGET ?? `http://localhost:${process.env.API_PORT ?? 4322}`

const caddy = spawn('caddy', ['run', '--config', 'Caddyfile', '--adapter', 'caddyfile'], {
  stdio: 'inherit',
  env: {
    ...process.env,
    WEB_PORT: port,
    WEB_ROOT: 'dist',
    CONFIG_ROOT: 'dist',
    // Localhost only, with /api forwarded to the local backend like `astro dev`.
    WEB_PREVIEW: `bind 127.0.0.1\nhandle /api/* {\nreverse_proxy ${api}\n}`,
  },
})

caddy.on('error', (error) => {
  console.error(
    error.code === 'ENOENT'
      ? 'caddy is not installed: run `mise install` (it is pinned in .mise.toml)'
      : `caddy failed to start: ${error.message}`
  )
  process.exit(1)
})

const stop = (signal) => () => {
  caddy.kill(signal)
}
process.on('SIGINT', stop('SIGINT'))
process.on('SIGTERM', stop('SIGTERM'))
caddy.on('exit', (code) => process.exit(code ?? 0))
