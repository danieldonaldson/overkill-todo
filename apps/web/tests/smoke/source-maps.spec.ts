import { expect, test } from '@playwright/test'
import { APP_BASE_URL } from './targets'

// Source maps are never served (OT-006, "Source maps are not public"). The web
// image keeps them out of /srv and the Caddyfile answers any `.map` with 404 (its
// SPA fallback would otherwise return index.html). Read-only: GETs only.
//
// Locally: `docker build -f apps/web/Dockerfile -t web .`, `docker run -p 8080:8080 web`,
// then `SMOKE_BASE_URL=http://localhost:8080 pnpm exec playwright test
// --config playwright.smoke.config.ts source-maps`.

const PAGES = ['/login', '/register', '/dashboard']

/** Every same-origin script a page loads. */
async function scriptsOf(request: import('@playwright/test').APIRequestContext, path: string) {
  const html = await (await request.get(`${APP_BASE_URL}${path}`)).text()
  const srcs = [...html.matchAll(/<script[^>]*\ssrc="([^"]+)"/g)].map((m) => m[1])
  return srcs.filter((src) => src.startsWith('/'))
}

test('no script references a source map, and every map path is a 404', async ({ request }) => {
  const scripts = new Set<string>()
  for (const page of PAGES) for (const src of await scriptsOf(request, page)) scripts.add(src)
  expect(scripts.size, 'found the site scripts').toBeGreaterThan(0)

  for (const src of scripts) {
    const script = await request.get(`${APP_BASE_URL}${src}`)
    expect(script.status(), src).toBe(200)
    expect(await script.text(), `${src} names no source map`).not.toContain('sourceMappingURL')
    expect(script.headers()['sourcemap'], `${src} has no SourceMap header`).toBeUndefined()

    const map = await request.get(`${APP_BASE_URL}${src.split('?')[0]}.map`)
    expect(map.status(), `${src}.map`).toBe(404)
  }
})

test('a made-up map path is a 404, not the app page', async ({ request }) => {
  const res = await request.get(`${APP_BASE_URL}/_astro/does-not-exist.js.map`)
  expect(res.status()).toBe(404)
})
