import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { expect, test, type Page } from '@playwright/test'
import { seedSession, TEST_USER } from './helpers/session'

// Web analytics (OT-005, specs/analytics). The site is served by `astro
// preview`, so this test stands in for the edge and the web container:
// - /config.js is rendered with (or without) a website ID, as the entrypoint would;
// - /stats/script.js serves the real Umami tracker (vendored from the pinned
//   image, see fixtures/) and /stats/api/send records what it sends;
// - every document gets the production CSP from apps/web/Caddyfile, so a
//   violation the tracker would cause shows up here.

const here = dirname(fileURLToPath(import.meta.url))
// Taken from the pinned image (`curl localhost:3001/script.js` with the analytics
// profile up). Refresh it when compose.yaml's Umami digest changes.
const TRACKER = readFileSync(join(here, 'fixtures', 'umami-tracker-3.5.0.js'), 'utf8')
const CSP = readFileSync(join(here, '..', 'Caddyfile'), 'utf8').match(
  /Content-Security-Policy\s+"([^"]*)"/
)![1]

const WEBSITE_ID = '4b0b8e0a-7a4c-4f53-9b1e-2f1d1c6a0e11'

interface Sent {
  type: string
  payload: { website: string; url: string; referrer?: string; name?: string }
}

interface Setup {
  websiteId?: string
  /** What the analytics service does with each /stats/* request. */
  stats?: 'ok' | 'abort' | 'slow'
  privacy?: 'gpc' | 'dnt'
}

async function setup(page: Page, { websiteId = WEBSITE_ID, stats = 'ok', privacy }: Setup = {}) {
  const sent: Sent[] = []
  const statsRequests: string[] = []
  const cspViolations: string[] = []

  page.on('console', (msg) => {
    if (msg.type() === 'error' && /content security policy/i.test(msg.text())) {
      cspViolations.push(msg.text())
    }
  })
  page.on('request', (req) => {
    if (new URL(req.url()).pathname.startsWith('/stats/')) statsRequests.push(req.url())
  })

  await page.addInitScript((privacy) => {
    document.addEventListener('securitypolicyviolation', (e) =>
      console.error(`Content Security Policy violation: ${e.violatedDirective} ${e.blockedURI}`)
    )
    if (privacy === 'gpc') {
      Object.defineProperty(Navigator.prototype, 'globalPrivacyControl', { get: () => true })
    }
    if (privacy === 'dnt') {
      Object.defineProperty(Navigator.prototype, 'doNotTrack', { get: () => '1' })
    }
  }, privacy)

  // Registered first, so the specific mocks below take precedence.
  await page.route('**/api/v1/**', (route) =>
    route.fulfill({ status: 404, json: { error: 'not_found', message: 'Not found' } })
  )
  await page.route('**/api/v1/auth/me', (route) => route.fulfill({ json: TEST_USER }))
  await page.route('**/api/v1/auth/register', (route) =>
    route.fulfill({ status: 201, json: { message: 'Check your email' } })
  )

  await page.route('**/config.js', (route) =>
    route.fulfill({
      contentType: 'application/javascript',
      body: `window.__PUBLIC_CONFIG = ${JSON.stringify({
        TURNSTILE_SITE_KEY: '',
        UMAMI_WEBSITE_ID: websiteId,
      })};`,
    })
  )

  await page.route('**/stats/**', async (route) => {
    if (stats === 'abort') return route.abort('connectionrefused')
    if (stats === 'slow') await new Promise((r) => setTimeout(r, 10_000))
    const path = new URL(route.request().url()).pathname
    if (path === '/stats/script.js') {
      return route.fulfill({ contentType: 'application/javascript', body: TRACKER })
    }
    if (path === '/stats/api/send') {
      sent.push(route.request().postDataJSON() as Sent)
      return route.fulfill({ json: {} })
    }
    return route.fulfill({ status: 404 })
  })

  await page.route('**/*', async (route) => {
    if (route.request().resourceType() !== 'document') return route.fallback()
    const response = await route.fetch()
    await route.fulfill({
      response,
      headers: { ...response.headers(), 'content-security-policy': CSP },
    })
  })

  const pageViews = () => sent.filter((s) => s.type === 'event' && !s.payload.name)
  return { sent, pageViews, statsRequests, cspViolations }
}

/** Gives the tracker time to send anything it was going to (it delays SPA page views 300 ms). */
async function settle(page: Page) {
  await page.waitForLoadState('networkidle')
  await page.waitForTimeout(800)
}

const PUBLIC_PAGES = [
  '/login',
  '/register',
  '/forgot-password',
  '/reset-password',
  '/verify-email',
  '/verify-magic',
  '/set-password',
]
const DASHBOARD_PAGES = ['/dashboard', '/dashboard/settings']

test.describe('web analytics', () => {
  test('with no website ID, no page loads the tracker or calls /stats/', async ({ page }) => {
    const { statsRequests } = await setup(page, { websiteId: '' })
    await seedSession(page)

    for (const path of [...PUBLIC_PAGES, ...DASHBOARD_PAGES]) {
      await page.goto(path)
      await settle(page)
    }

    expect(statsRequests).toEqual([])
    await expect(page.locator('script[src="/stats/script.js"]')).toHaveCount(0)
  })

  test('an unrendered placeholder ID counts as off', async ({ page }) => {
    const { statsRequests } = await setup(page, { websiteId: '${PUBLIC_UMAMI_WEBSITE_ID}' })

    await page.goto('/register')
    await settle(page)

    expect(statsRequests).toEqual([])
  })

  test('opening Register sends one page view, path only, to our own origin', async ({ page }) => {
    const { pageViews, statsRequests } = await setup(page)

    await page.goto('/register')
    await settle(page)

    expect(pageViews()).toHaveLength(1)
    const view = pageViews()[0].payload
    expect(view.website).toBe(WEBSITE_ID)
    const url = new URL(view.url)
    expect(url.pathname).toBe('/register')
    expect(url.search).toBe('')
    expect(url.hash).toBe('')

    const origin = new URL(page.url()).origin
    expect(statsRequests.length).toBeGreaterThan(0)
    for (const req of statsRequests) expect(new URL(req).origin).toBe(origin)
  })

  test('tokens and codes in the query string or fragment are never sent', async ({ page }) => {
    const { sent, pageViews } = await setup(page)

    await page.goto('/verify-magic?token=abc-secret-7f3#frag-x')
    await settle(page)

    expect(pageViews().length).toBeGreaterThan(0)
    const everything = JSON.stringify(sent)
    expect(everything).not.toContain('abc-secret-7f3')
    expect(everything).not.toContain('frag-x')
    expect(everything).not.toContain('token=')
  })

  test('moving Home → Settings in the dashboard sends two page views', async ({ page }) => {
    await seedSession(page)
    const { pageViews } = await setup(page)

    await page.goto('/dashboard')
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await settle(page)
    await page.getByRole('link', { name: 'Settings' }).click()
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await settle(page)

    expect(pageViews().map((v) => new URL(v.payload.url).pathname)).toEqual([
      '/dashboard',
      '/dashboard/settings',
    ])
  })

  test('registration carries no email address into analytics', async ({ page }) => {
    const { sent } = await setup(page)

    await page.goto('/register')
    await page.getByLabel(/email/i).fill('visitor@example.com')
    await page.getByRole('button', { name: /continue/i }).click()
    // The next page carries the address in its query string.
    await page.waitForURL('**/verify-email?email=*')
    await settle(page)

    expect(JSON.stringify(sent)).not.toContain('visitor')
  })

  for (const stats of ['abort', 'slow'] as const) {
    test(`registration works as normal when analytics is ${stats === 'abort' ? 'down' : 'slow'}`, async ({
      page,
    }) => {
      await setup(page, { stats })

      await page.goto('/register')
      await page.getByLabel(/email/i).fill('visitor@example.com')
      const started = Date.now()
      await page.getByRole('button', { name: /continue/i }).click()

      await page.waitForURL('**/verify-email?email=*', { timeout: 3_000 })
      expect(Date.now() - started).toBeLessThan(3_000)
      await expect(page.getByRole('alert')).toHaveCount(0)
    })
  }

  for (const privacy of ['gpc', 'dnt'] as const) {
    test(`a browser sending ${privacy.toUpperCase()} makes no analytics request`, async ({
      page,
    }) => {
      const { statsRequests } = await setup(page, { privacy })

      // A Visitor signs up...
      await page.goto('/register')
      await page.getByLabel(/email/i).fill('visitor@example.com')
      await page.getByRole('button', { name: /continue/i }).click()
      await page.waitForURL('**/verify-email?email=*')
      // ...and a Member uses the dashboard (seeded only now: a signed-in
      // browser is sent from /register to the dashboard).
      await seedSession(page)
      await page.goto('/dashboard')
      await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
      await page.getByRole('link', { name: 'Settings' }).click()
      await settle(page)

      expect(statsRequests).toEqual([])
    })
  }

  test('analytics sets no cookie and writes no browser storage', async ({ page, context }) => {
    const { pageViews } = await setup(page)

    await page.goto('/login')
    await settle(page)
    await page.goto('/register')
    await settle(page)

    expect(pageViews().length).toBeGreaterThan(0)
    expect(await context.cookies()).toEqual([])
    const keys = await page.evaluate(() => [
      ...Object.keys(localStorage),
      ...Object.keys(sessionStorage),
    ])
    expect(keys.filter((k) => /umami|analytics|stats/i.test(k))).toEqual([])
  })

  test('no page logs a CSP violation with analytics on', async ({ page }) => {
    await seedSession(page)
    const { pageViews, cspViolations } = await setup(page)

    for (const path of [...PUBLIC_PAGES, ...DASHBOARD_PAGES]) {
      await page.goto(path)
      await settle(page)
    }

    expect(pageViews().length).toBeGreaterThan(0)
    expect(cspViolations).toEqual([])
  })
})
