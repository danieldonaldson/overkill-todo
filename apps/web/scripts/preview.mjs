// Local preview through the same Caddyfile the web image runs (design D8):
// `astro preview` intentionally drops user Vite plugins, so it cannot serve
// the one shell for every list address. Playwright therefore runs against the
// web server configuration, rooted at the built site.
import { spawn } from 'node:child_process'

const args = process.argv.slice(2)
const portIndex = args.indexOf('--port')
const port = portIndex === -1 ? '4323' : (args[portIndex + 1] ?? '4323')

const caddy = spawn('caddy', ['run', '--config', 'Caddyfile', '--adapter', 'caddyfile'], {
  stdio: 'inherit',
  env: {
    ...process.env,
    WEB_PORT: port,
    WEB_ROOT: 'dist',
    CONFIG_ROOT: 'dist',
  },
})

const stop = (signal) => () => {
  caddy.kill(signal)
}
process.on('SIGINT', stop('SIGINT'))
process.on('SIGTERM', stop('SIGTERM'))
caddy.on('exit', (code) => process.exit(code ?? 0))
