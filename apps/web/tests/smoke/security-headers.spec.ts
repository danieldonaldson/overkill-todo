import { expect, test, type APIRequestContext } from '@playwright/test'
import { APP_BASE_URL, REQUIRED_SECURITY_HEADERS, parseCsp } from './targets'

// The reason this suite exists.
//
// nginx `add_header` inheritance is REPLACE, not merge: any location block that
// declares an add_header of its own drops every server-level one. A block that
// set only Cache-Control therefore stripped CSP, Permissions-Policy and COOP
// from every HTML response in production, while nginx.conf still read as though
// CSP were enforced. The repo's csp.spec.ts and nginx-headers.spec.ts guard the
// *file*; this guards the *bytes a live edge returns*, for every distinct class
// of response nginx routes differently. That is the only place the bug is
// visible.

// Hashed asset filenames change every build, so discover them from the live
// HTML instead of pinning them.
async function discoverAssets(request: APIRequestContext): Promise<{ js: string; css: string }> {
  const html = await (await request.get(`${APP_BASE_URL}/login/`)).text()
  const js = html.match(/src="(\/_astro\/[^"]+\.js)"/)?.[1]
  const css = html.match(/href="(\/_astro\/[^"]+\.css)"/)?.[1]
  expect(js, 'live HTML references a hashed /_astro/ script').toBeTruthy()
  expect(css, 'live HTML references a hashed /_astro/ stylesheet').toBeTruthy()
  return { js: js!, css: css! }
}

test.describe('security headers on live responses', () => {
  // Each entry is a response class nginx can route through a different location
  // block — i.e. a separate opportunity to shadow the headers.
  test('every class of dashboard response carries the full header set', async ({ request }) => {
    const { js, css } = await discoverAssets(request)
    const paths = [
      { path: '/', why: 'HTML document (server-level location)' },
      { path: '/login/', why: 'prerendered route' },
      { path: '/a-path-that-does-not-exist', why: 'SPA fallback' },
      { path: js, why: 'hashed, immutably-cached script' },
      { path: css, why: 'hashed, immutably-cached stylesheet' },
      { path: '/config.js', why: 'runtime config, no-store' },
      { path: '/scripts/theme-init.js', why: 'static script with its own TTL' },
    ]

    const missing: string[] = []
    for (const { path, why } of paths) {
      const res = await request.get(`${APP_BASE_URL}${path}`)
      expect(res.status(), `${path} is served`).toBe(200)
      const headers = res.headers()
      for (const name of REQUIRED_SECURITY_HEADERS) {
        if (!headers[name]) missing.push(`${path} (${why}) is missing ${name}`)
      }
    }

    expect(
      missing,
      'a location block is shadowing the server-level add_headers — see nginx-headers.spec.ts',
    ).toEqual([])
  })

})

test.describe('CSP content, as actually served', () => {
  test('the dashboard pins inline scripts by hash rather than allowing unsafe-inline', async ({
    request,
  }) => {
    const res = await request.get(`${APP_BASE_URL}/`)
    const csp = parseCsp(res.headers()['content-security-policy'])
    const scriptSrc = csp.get('script-src')
    expect(scriptSrc, 'CSP declares script-src').toBeTruthy()

    expect(scriptSrc).not.toContain("'unsafe-inline'")
    // Hashes are what makes hydration work without unsafe-inline; zero of them
    // means the build changed and nobody re-pinned.
    expect(
      scriptSrc!.filter((s) => s.startsWith("'sha256-")).length,
      'inline scripts are pinned by hash',
    ).toBeGreaterThan(0)
  })

  // A CSP that is too tight breaks the product silently, in production. Pin
  // every third-party origin a user-facing flow depends on.
  const REQUIRED_ORIGINS = [
    {
      origin: 'https://challenges.cloudflare.com',
      directives: ['script-src', 'frame-src'],
      why: 'Turnstile on login/register: api.js plus the widget iframe',
    },
  ]

  test('every third-party origin a user-facing flow depends on is allowed', async ({ request }) => {
    const res = await request.get(`${APP_BASE_URL}/`)
    const csp = parseCsp(res.headers()['content-security-policy'])

    const missing: string[] = []
    for (const { origin, directives, why } of REQUIRED_ORIGINS) {
      for (const name of directives) {
        if (!(csp.get(name) ?? []).includes(origin)) {
          missing.push(`${name} is missing ${origin} — ${why}`)
        }
      }
    }
    expect(missing, 'removing these breaks a user-facing flow in production').toEqual([])
  })

  test('the CSP forbids the directives that make clickjacking and injection cheap', async ({
    request,
  }) => {
    const res = await request.get(`${APP_BASE_URL}/`)
    const csp = parseCsp(res.headers()['content-security-policy'])
    expect(csp.get('object-src')).toEqual(["'none'"])
    expect(csp.get('base-uri')).toEqual(["'self'"])
    expect(csp.get('form-action')).toEqual(["'self'"])
    expect(csp.get('frame-ancestors'), 'frame-ancestors is set').toBeTruthy()
  })
})
