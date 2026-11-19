import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { devices, expect, test, type Page } from '@playwright/test'
import { TEST_USER } from './helpers/session'

// Browser error reports (OT-006 slice 1, specs/observability). The API is
// mocked: /api/v1/client-telemetry records each beacon. Every document gets the
// production CSP from apps/web/Caddyfile, so a violation the reporter caused
// would show up here.

const here = dirname(fileURLToPath(import.meta.url))
const CSP = readFileSync(join(here, '..', 'Caddyfile'), 'utf8').match(
  /Content-Security-Policy\s+"([^"]*)"/
)![1]

interface Report {
  page_load_id: string
  release: string
  page: string
  events: Array<{
    type: string
    kind: string
    message: string
    stack?: string
    source?: string
    count: number
  }>
}

interface Setup {
  /** What happens to report requests. */
  endpoint?: 'ok' | 'abort'
}

async function setup(page: Page, { endpoint = 'ok' }: Setup = {}) {
  const reports: Report[] = []
  const rawBodies: string[] = []
  const consoleErrors: string[] = []
  const cspViolations: string[] = []

  page.on('console', (msg) => {
    if (msg.type() !== 'error') return
    if (/content security policy/i.test(msg.text())) cspViolations.push(msg.text())
    else consoleErrors.push(msg.text())
  })
  await page.addInitScript(() => {
    document.addEventListener('securitypolicyviolation', (e) =>
      console.error(`Content Security Policy violation: ${e.violatedDirective} ${e.blockedURI}`)
    )
  })

  // Registered first, so the specific mocks below take precedence.
  await page.route('**/api/v1/**', (route) =>
    route.fulfill({ status: 404, json: { code: 'not_found', message: 'Not found' } })
  )
  await page.route('**/api/v1/auth/me', (route) => route.fulfill({ json: TEST_USER }))
  await page.route('**/api/v1/auth/login', (route) =>
    route.fulfill({
      json: {
        user: TEST_USER,
        expires_at: Math.floor(Date.now() / 1000) + 900,
        csrf_token: 'csrf',
      },
    })
  )
  await page.route('**/api/v1/client-telemetry', (route) => {
    if (endpoint === 'abort') return route.abort('connectionrefused')
    const body = route.request().postData() ?? ''
    rawBodies.push(body)
    reports.push(JSON.parse(body) as Report)
    return route.fulfill({ status: 204 })
  })

  await page.route('**/*', async (route) => {
    if (route.request().resourceType() !== 'document') return route.fallback()
    const response = await route.fetch()
    await route.fulfill({
      response,
      headers: { ...response.headers(), 'content-security-policy': CSP },
    })
  })

  const events = () => reports.flatMap((r) => r.events)
  return { reports, rawBodies, events, consoleErrors, cspViolations }
}

/** Throw `count` uncaught errors from page code (outside `evaluate`'s own catch). */
async function throwErrors(page: Page, count: number, distinct: boolean, prefix = 'boom') {
  await page.evaluate(
    ({ count, distinct, prefix }) => {
      for (let i = 0; i < count; i++) {
        setTimeout(() => {
          throw new Error(distinct ? `${prefix} ${i}` : prefix)
        })
      }
    },
    { count, distinct, prefix }
  )
  // Let every timer fire.
  await page.evaluate(() => new Promise((r) => setTimeout(r, 50)))
}

/** The page is hidden (tab switch, app switch on a phone): the reporter flushes. */
async function hide(page: Page) {
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true })
    document.dispatchEvent(new Event('visibilitychange'))
  })
}

/** Wait for the reporter script and hydration to settle. */
async function ready(page: Page) {
  await page.waitForLoadState('networkidle')
}

test.describe('browser error reports', () => {
  test('an error on Login sends one report with the page, release, kind and a stack', async ({
    page,
  }) => {
    const { reports, events } = await setup(page)
    await page.goto('/login')
    await ready(page)

    await throwErrors(page, 1, false, 'login broke')
    await hide(page)

    await expect.poll(() => reports.length).toBe(1)
    expect(reports[0].page).toBe('/login')
    expect(reports[0].release).toMatch(/^[0-9A-Za-z._-]{1,64}$/)
    expect(reports[0].page_load_id).toMatch(/^[0-9a-f]{32}$/)
    expect(events()).toHaveLength(1)
    expect(events()[0]).toMatchObject({
      type: 'error',
      kind: 'error',
      message: 'Error: login broke',
      count: 1,
    })
    expect(events()[0].stack).toContain('login broke')
  })

  test('an unhandled rejection is reported with its kind', async ({ page }) => {
    const { events } = await setup(page)
    await page.goto('/login')
    await ready(page)

    await page.evaluate(() => {
      void Promise.reject(new Error('nobody caught me'))
    })
    await page.evaluate(() => new Promise((r) => setTimeout(r, 50)))
    await hide(page)

    await expect
      .poll(() => events().map((e) => [e.kind, e.message]))
      .toEqual([['unhandledrejection', 'Error: nobody caught me']])
  })

  for (const { name, path, secrets } of [
    {
      name: 'the reset page (email in the query string)',
      path: '/reset-password?email=a@b.c',
      secrets: ['a@b.c', 'a%40b.c', 'email='],
    },
    {
      name: 'a magic link (token in the fragment)',
      path: '/verify-magic#token=mlk_s3cr3tT0k3n',
      secrets: ['mlk_s3cr3tT0k3n', 'token='],
    },
  ]) {
    test(`on ${name}, an error holding the full address sends neither secret`, async ({ page }) => {
      const { rawBodies, reports } = await setup(page)
      // The address as opened: the magic-link page removes the fragment from
      // the address bar once it has read the token.
      await page.addInitScript(() => {
        ;(window as unknown as { __openedHref: string }).__openedHref = location.href
      })
      await page.goto(path)
      await ready(page)

      await page.evaluate(() => {
        const href = (window as unknown as { __openedHref: string }).__openedHref
        setTimeout(() => {
          const error = new Error(`failed at ${href}`)
          error.stack = `Error: failed at ${href}\n    at f (${href}:1:2)`
          throw error
        })
      })
      await page.evaluate(() => new Promise((r) => setTimeout(r, 50)))
      await hide(page)

      await expect.poll(() => reports.length).toBeGreaterThan(0)
      const everything = rawBodies.join('\n')
      for (const secret of secrets) expect(everything).not.toContain(secret)
      expect(reports[0].page).toBe(new URL(path, 'http://x').pathname)
    })
  }

  test('the same error 50 times is one event with count 50', async ({ page }) => {
    const { events } = await setup(page)
    await page.goto('/login')
    await ready(page)

    await throwErrors(page, 50, false)
    await hide(page)

    await expect.poll(() => events().length).toBe(1)
    expect(events()[0].count).toBe(50)
  })

  test('500 different errors give at most 20 events', async ({ page }) => {
    const { events, reports } = await setup(page)
    await page.goto('/login')
    await ready(page)

    await throwErrors(page, 500, true)
    await hide(page)

    await expect.poll(() => events().length).toBe(20)
    // Two batches at most (30 events per batch), all from one page load.
    expect(new Set(reports.map((r) => r.page_load_id)).size).toBe(1)
  })

  test('with the endpoint blocked, sign-in still works and the reporter logs nothing', async ({
    page,
  }) => {
    const { consoleErrors } = await setup(page, { endpoint: 'abort' })
    await page.goto('/login')
    await ready(page)
    await throwErrors(page, 3, true)
    await hide(page)

    await page.getByLabel(/email/i).fill('member@example.com')
    await page.getByRole('textbox', { name: /password/i }).fill('correct horse battery')
    await page.getByRole('button', { name: /^sign in$/i }).click()
    await page.waitForURL('**/dashboard')

    // Chromium logs the blocked request itself ("Failed to load resource"); the
    // thrown test errors appear as page errors, not console errors. Nothing else.
    const fromReporter = consoleErrors.filter((t) => !/Failed to load resource/i.test(t))
    expect(fromReporter).toEqual([])
  })

  test('reporting on every page causes no CSP violation', async ({ page }) => {
    const { cspViolations, reports } = await setup(page)
    for (const path of ['/login', '/register', '/forgot-password', '/verify-email']) {
      await page.goto(path)
      await ready(page)
      await throwErrors(page, 1, false, `on ${path}`)
      await hide(page)
    }
    await expect.poll(() => reports.length).toBe(4)
    expect(cspViolations).toEqual([])
  })
})

test.describe('on a phone', () => {
  // Pixel 7's viewport, touch and user agent, on the configured Chromium (a
  // describe can't switch browser type).
  const { viewport, userAgent, deviceScaleFactor, isMobile, hasTouch } = devices['Pixel 7']
  test.use({ viewport, userAgent, deviceScaleFactor, isMobile, hasTouch })

  test('switching away flushes what was collected', async ({ page }) => {
    const { reports } = await setup(page)
    await page.goto('/register')
    await ready(page)

    await throwErrors(page, 2, true)
    expect(reports).toHaveLength(0)
    await hide(page)

    await expect.poll(() => reports.length).toBe(1)
    expect(reports[0].page).toBe('/register')
    expect(reports[0].events).toHaveLength(2)
  })
})
