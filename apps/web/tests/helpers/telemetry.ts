import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import type { Page } from '@playwright/test'
import { seedSession, TEST_USER } from './session'

// Shared by the browser telemetry flows (OT-006): the API is mocked, and
// /api/v1/client-telemetry records each report. Every document gets the
// production CSP from apps/web/Caddyfile, so a violation the reporter caused
// shows up.

const here = dirname(fileURLToPath(import.meta.url))
const CSP = readFileSync(join(here, '..', '..', 'Caddyfile'), 'utf8').match(
  /Content-Security-Policy\s+"([^"]*)"/
)![1]

export interface ReportEvent {
  type: string
  kind: string
  message: string
  stack?: string
  source?: string
  island?: string
  api_route?: string
  status?: number
  network_failed?: boolean
  trace_id?: string
  count: number
  /** For `vital` events. */
  name?: string
  value?: number
  rating?: string
}

export interface Report {
  page_load_id: string
  release: string
  page: string
  events: ReportEvent[]
}

export interface Setup {
  /** What happens to report requests. */
  endpoint?: 'ok' | 'abort'
  /** Signed in as TEST_USER (merged with `user`), for dashboard pages. */
  signedIn?: boolean
  user?: object
  /** The island to crash on purpose, from before the page loads. */
  crash?: string
  /** `RUM_SAMPLE_RATE` in the rendered /config.js; unset means all page loads. */
  sampleRate?: string
}

export async function setup(
  page: Page,
  { endpoint = 'ok', signedIn, user, crash, sampleRate }: Setup = {}
) {
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

  if (signedIn) await seedSession(page)
  if (crash) {
    await page.addInitScript((island) => {
      ;(window as unknown as { __otCrashIsland?: string }).__otCrashIsland = island
    }, crash)
  }

  // Registered first, so the specific mocks below take precedence.
  await page.route('**/api/v1/**', (route) =>
    route.fulfill({ status: 404, json: { code: 'not_found', message: 'Not found' } })
  )
  await page.route('**/api/v1/auth/me', (route) =>
    route.fulfill({ json: { ...TEST_USER, ...user } })
  )
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

  if (sampleRate !== undefined) {
    await page.route('**/config.js', (route) =>
      route.fulfill({
        contentType: 'application/javascript',
        body: `window.__PUBLIC_CONFIG = ${JSON.stringify({
          TURNSTILE_SITE_KEY: '',
          UMAMI_WEBSITE_ID: '',
          RUM_SAMPLE_RATE: sampleRate,
        })};`,
      })
    )
  }

  await page.route('**/*', async (route) => {
    if (route.request().resourceType() !== 'document') return route.fallback()
    const response = await route.fetch()
    await route.fulfill({
      response,
      headers: { ...response.headers(), 'content-security-policy': CSP },
    })
  })

  /** Error events, in order (page loads left out). */
  const events = () => reports.flatMap((r) => r.events).filter((e) => e.type === 'error')
  /** `[page, page_load_id]` of each page load reported, in order. */
  const pageLoads = () =>
    reports.flatMap((r) =>
      r.events.filter((e) => e.type === 'page_load').map(() => [r.page, r.page_load_id])
    )
  /** `[name, page]` of each Web Vital reported, in order. */
  const vitals = () =>
    reports.flatMap((r) =>
      r.events.filter((e) => e.type === 'vital').map((e) => [e.name!, r.page] as const)
    )
  return { reports, rawBodies, events, pageLoads, vitals, consoleErrors, cspViolations }
}

/** Throw `count` uncaught errors from page code (outside `evaluate`'s own catch). */
export async function throwErrors(page: Page, count: number, distinct: boolean, prefix = 'boom') {
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
export async function hide(page: Page) {
  await page.evaluate(() => {
    Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true })
    document.dispatchEvent(new Event('visibilitychange'))
  })
}

/** Wait for the reporter script and hydration to settle. */
export async function ready(page: Page) {
  await page.waitForLoadState('networkidle')
}

