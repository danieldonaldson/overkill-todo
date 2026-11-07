import type { APIRoute } from 'astro'

// Probe target for the Tilt inner loop, where the pod runs `astro dev` instead of the
// nginx serve stage (which answers /healthz itself — see apps/web/nginx.conf).
// Prerendered under `output: 'static'`, so the built image carries it too.
export const prerender = true

export const GET: APIRoute = () =>
  new Response('ok\n', { headers: { 'content-type': 'text/plain' } })
