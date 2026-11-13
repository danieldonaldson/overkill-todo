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
  payload: {
    website: string
    url: string
    referrer?: string
    name?: string
    data?: Record<string, unknown>
  }
}

// The spec's event table: each event and the only values its property may take.
const EVENT_TABLE: Record<string, Record<string, string[]>> = {
  'signup-submitted': {},
  'signup-rejected': { reason: ['invalid', 'captcha', 'rate-limited', 'server'] },
  'email-verified': { via: ['code', 'link'] },
  'signin-succeeded': {},
  'signin-rejected': { reason: ['credentials', 'captcha', 'rate-limited', 'server'] },
  'oauth-started': { provider: ['google'] },
  'password-reset-requested': {},
  'password-reset-completed': {},
  'account-deletion-requested': {},
}

/** Every named event sent is in the table, with exactly its properties and allowed values. */
function expectOnlyTableEvents(sent: Sent[]) {
  for (const { payload } of sent.filter((s) => s.payload.name)) {
    const allowed = EVENT_TABLE[payload.name!]
    expect(allowed, `event ${payload.name} is in the spec's table`).toBeDefined()
    const data = payload.data ?? {}
    expect(Object.keys(data).sort()).toEqual(Object.keys(allowed).sort())
    for (const [key, value] of Object.entries(data)) {
      expect(allowed[key], `${payload.name}.${key}`).toContain(value)
    }
  }
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
  const events = () => sent.filter((s) => s.payload.name)
  return { sent, pageViews, events, statsRequests, cspViolations }
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

  test('sign-up funnel: page view, signup-submitted, then email-verified via code', async ({
    page,
  }) => {
    const { sent } = await setup(page)
    await page.route('**/api/v1/auth/verify-email', (route) =>
      route.fulfill({ json: { message: 'Verified', signup_token: 'signup-tok-123' } })
    )

    await page.goto('/register')
    await page.getByLabel(/email/i).fill('visitor@example.com')
    await page.getByRole('button', { name: /continue/i }).click()
    await page.waitForURL('**/verify-email?email=*')
    // Typed before hydration, the code would be reset by the island.
    await settle(page)
    await page.getByLabel('Verification code').pressSequentially('123456')
    await page.waitForURL('**/set-password')
    await settle(page)

    const steps = sent.map((s) =>
      s.payload.name
        ? `${s.payload.name}${s.payload.data ? ` ${JSON.stringify(s.payload.data)}` : ''}`
        : `view ${new URL(s.payload.url).pathname}`
    )
    const funnel = steps.filter((s) =>
      ['view /register', 'signup-submitted', 'email-verified {"via":"code"}'].includes(s)
    )
    expect(funnel).toEqual(['view /register', 'signup-submitted', 'email-verified {"via":"code"}'])
    expect(JSON.stringify(sent)).not.toContain('signup-tok-123')
    expectOnlyTableEvents(sent)
  })

  test('a sign-up the server rejects sends signup-rejected invalid, without the address or message', async ({
    page,
  }) => {
    const { sent, events } = await setup(page)
    await page.route('**/api/v1/auth/register', (route) =>
      route.fulfill({
        status: 400,
        json: { code: 'validation_error', message: 'No mail server for nomx.example' },
      })
    )

    await page.goto('/register')
    await page.getByLabel(/email/i).fill('visitor@nomx.example')
    await page.getByRole('button', { name: /continue/i }).click()
    await expect(page.getByText('No mail server for nomx.example')).toBeVisible()
    await settle(page)

    expect(events().map((e) => [e.payload.name, e.payload.data])).toEqual([
      ['signup-rejected', { reason: 'invalid' }],
    ])
    const everything = JSON.stringify(sent)
    expect(everything).not.toContain('nomx')
    expect(everything).not.toContain('mail server')
    expectOnlyTableEvents(sent)
  })

  test('a failed bot check is reported as captcha', async ({ page }) => {
    const { events } = await setup(page)
    await page.route('**/api/v1/auth/login', (route) =>
      route.fulfill({
        status: 400,
        json: { code: 'bot_check_failed', message: 'Bot verification failed. Please try again.' },
      })
    )

    await page.goto('/login')
    await page.getByLabel(/email/i).fill('member@example.com')
    await page.getByRole('textbox', { name: /password/i }).fill('correct horse battery')
    await page.getByRole('button', { name: /^sign in$/i }).click()
    await expect(page.getByText('Bot verification failed')).toBeVisible()
    await settle(page)

    expect(events().map((e) => [e.payload.name, e.payload.data])).toEqual([
      ['signin-rejected', { reason: 'captcha' }],
    ])
  })

  test('sign-in with the server unreachable sends signin-rejected server and shows the usual error', async ({
    page,
  }) => {
    const { sent, events } = await setup(page)
    await page.route('**/api/v1/auth/login', (route) => route.abort('connectionrefused'))

    await page.goto('/login')
    await page.getByLabel(/email/i).fill('member@example.com')
    await page.getByRole('textbox', { name: /password/i }).fill('correct horse battery')
    await page.getByRole('button', { name: /^sign in$/i }).click()
    await expect(page.getByRole('alert')).toBeVisible()
    await settle(page)

    expect(events().map((e) => [e.payload.name, e.payload.data])).toEqual([
      ['signin-rejected', { reason: 'server' }],
    ])
    expect(JSON.stringify(sent)).not.toContain('member@example.com')
    expectOnlyTableEvents(sent)
  })

  test('Continue with Google sends oauth-started before the browser leaves', async ({ page }) => {
    const { sent, events } = await setup(page)
    await page.route('**/api/v1/auth/oauth/device-fingerprint', (route) =>
      route.fulfill({ status: 204 })
    )

    await page.goto('/login')
    await settle(page)
    const leaving = page.waitForRequest('**/api/v1/auth/oauth/google?mode=login')
    await page.getByRole('button', { name: /google/i }).click()
    await leaving

    await expect
      .poll(() => events().map((e) => [e.payload.name, e.payload.data]))
      .toEqual([['oauth-started', { provider: 'google' }]])
    expectOnlyTableEvents(sent)
  })

  test('with analytics blocked, Google sign-in still leaves for the provider', async ({ page }) => {
    await setup(page, { stats: 'abort' })
    await page.route('**/api/v1/auth/oauth/device-fingerprint', (route) =>
      route.fulfill({ status: 204 })
    )

    await page.goto('/login')
    const leaving = page.waitForRequest('**/api/v1/auth/oauth/google?mode=login')
    await page.getByRole('button', { name: /google/i }).click()
    await leaving
  })

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
