import { devices, expect, test, type Page } from '@playwright/test'
import { hide, ready, settled, setup } from './helpers/telemetry'

// In-site navigation and panel timing (OT-006 slice 4, specs/observability
// "In-site navigations are measured" and "Panel loading is measured"). The API
// is mocked; /api/v1/client-telemetry records each report. Chromium, where
// view transitions run: re-run this on every Astro upgrade (design Risks).

const ADMIN = { role: 'admin' }
const USERS = { users: [], total: 0, limit: 50, offset: 0 }

/** Answer the admin user list (or hold it until `release` is called). */
async function mockUsers(page: Page, how: 'ok' | 'fail' | 'hold' = 'ok') {
  let release = () => {}
  const held = new Promise<void>((r) => {
    release = r
  })
  await page.route('**/api/v1/admin/users*', async (route) => {
    if (how === 'fail') {
      return route.fulfill({ status: 500, json: { code: 'internal', message: 'Internal error' } })
    }
    if (how === 'hold') await held
    return route.fulfill({ json: USERS })
  })
  return { release }
}

/** Hold the router's fetch of `path` until `release` is called. */
async function holdPageFetch(page: Page, path: string) {
  let release = () => {}
  const held = new Promise<void>((r) => {
    release = r
  })
  await page.route(`**${path}`, async (route) => {
    if (route.request().resourceType() !== 'document') await held
    await route.fallback()
  })
  return { release }
}

const nav = (page: Page, label: string) =>
  page.getByRole('navigation').getByRole('link', { name: label, exact: true })

/** Timings present and ordered: phases fit inside the total. */
function expectOrdered(n: {
  total_ms?: number
  fetch_ms?: number
  swap_ms?: number
  transition_ms?: number
}) {
  expect(n.total_ms).toBeGreaterThan(0)
  expect(n.fetch_ms).toBeGreaterThanOrEqual(0)
  expect(n.swap_ms).toBeGreaterThanOrEqual(0)
  expect(n.fetch_ms! + n.swap_ms!).toBeLessThanOrEqual(n.total_ms!)
  if (n.transition_ms !== undefined) expect(n.transition_ms).toBeLessThanOrEqual(n.total_ms!)
}

test.describe('Navigation timing', () => {
  test('Home → Settings, back, then Settings interrupted by Admin', async ({ page }) => {
    const t = await setup(page, { signedIn: true, user: ADMIN })
    await mockUsers(page)
    await page.goto('/dashboard')
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await expect(nav(page, 'Admin')).toBeVisible()
    await ready(page)

    await nav(page, 'Settings').click()
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await settled(page)

    await page.goBack()
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await settled(page)

    // Settings is slow to arrive; the Member clicks Admin before it does.
    const settings = await holdPageFetch(page, '/dashboard/settings')
    const fetching = page.waitForRequest('**/dashboard/settings')
    await nav(page, 'Settings').click()
    await fetching
    await nav(page, 'Admin').click()
    await expect(page.getByRole('heading', { name: 'Admin' })).toBeVisible()
    settings.release()
    await settled(page)
    await hide(page)

    await expect.poll(() => t.navigations().length).toBe(5)
    // One flush groups events by page, so compare without order.
    const summary = t.navigations().map((n) => `${n.page} ${n.nav_type} ${n.outcome}`)
    expect(summary.sort()).toEqual([
      '/dashboard hard completed',
      '/dashboard soft completed',
      '/dashboard/admin soft completed',
      '/dashboard/settings soft aborted',
      '/dashboard/settings soft completed',
    ])
    const find = (page: string, type: string, outcome: string) =>
      t.navigations().find((n) => n.page === page && n.nav_type === type && n.outcome === outcome)!
    const hard = find('/dashboard', 'hard', 'completed')
    const toSettings = find('/dashboard/settings', 'soft', 'completed')
    const back = find('/dashboard', 'soft', 'completed')
    const aborted = find('/dashboard/settings', 'soft', 'aborted')
    const toAdmin = find('/dashboard/admin', 'soft', 'completed')
    expect(hard).toMatchObject({ view_transition: 'none' })
    expect(hard.fetch_ms).toBeUndefined()
    for (const soft of [toSettings, back, toAdmin]) {
      expect(soft.view_transition).toBe('ran')
      expectOrdered(soft)
      expect(soft.transition_ms).toBeGreaterThan(0)
    }
    expect(aborted.total_ms).toBeUndefined()

    // The persisted header is timed once, on the full load.
    const header = t.islands().filter((i) => i.name === 'DashboardHeader')
    expect(header.map((i) => [i.stage, i.nav_type, i.page]).sort()).toEqual([
      ['interactive', 'hard', '/dashboard'],
      ['ready', 'hard', '/dashboard'],
    ])
    // Each panel is timed for the navigation that showed it.
    const panels = t
      .islands()
      .filter((i) => i.name !== 'DashboardHeader' && i.name !== 'ModeWatcher')
      .map((i) => [i.name, i.stage, i.nav_type])
    expect(panels).toEqual(
      expect.arrayContaining([
        ['HomePanel', 'interactive', 'hard'],
        ['HomePanel', 'ready', 'hard'],
        ['SettingsPanel', 'interactive', 'soft'],
        ['SettingsPanel', 'ready', 'soft'],
        ['HomePanel', 'ready', 'soft'],
        ['AdminPanel', 'interactive', 'soft'],
        ['AdminPanel', 'ready', 'soft'],
      ])
    )
    for (const i of t.islands()) expect(i.ms).toBeGreaterThanOrEqual(0)
    expect(t.cspViolations).toEqual([])
  })

  test('a browser without view transitions still reports, as unsupported', async ({ page }) => {
    await page.addInitScript(() => {
      delete (Document.prototype as { startViewTransition?: unknown }).startViewTransition
    })
    const t = await setup(page, { signedIn: true })
    await page.goto('/dashboard')
    await ready(page)
    await nav(page, 'Settings').click()
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await settled(page)
    await hide(page)

    await expect
      .poll(() => t.navigations().map((n) => [n.page, n.view_transition]))
      .toEqual(
        expect.arrayContaining([
          ['/dashboard', 'none'],
          ['/dashboard/settings', 'unsupported'],
        ])
      )
    expect(t.navigations()).toHaveLength(2)
    const soft = t.navigations().find((n) => n.page === '/dashboard/settings')!
    expectOrdered(soft)
    expect(soft.transition_ms).toBeUndefined()
  })

  test('a navigation that falls back to a full load is reported as a full load', async ({
    page,
  }) => {
    const t = await setup(page, { signedIn: true })
    // The router's fetch gets a page without the ClientRouter, so Astro gives
    // up on the swap and loads the page in full.
    await page.route('**/dashboard/settings', (route) =>
      route.request().resourceType() === 'document'
        ? route.fallback()
        : route.fulfill({ contentType: 'text/html', body: '<!doctype html><title>x</title>' })
    )
    await page.goto('/dashboard')
    await ready(page)
    await nav(page, 'Settings').click()
    await page.waitForURL('**/dashboard/settings')
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await settled(page)
    await hide(page)

    // The old document's queue goes out in a beacon as it unloads, which the
    // mock can't always catch; what matters is how Settings was reported.
    await expect
      .poll(() =>
        t
          .navigations()
          .filter((n) => n.page === '/dashboard/settings')
          .map((n) => [n.nav_type, n.outcome])
      )
      .toEqual([['hard', 'completed']])
    expect(t.navigations().filter((n) => n.nav_type === 'soft')).toEqual([])
  })
})

test.describe('Panel timing', () => {
  test('a panel whose data call fails is ready at its error state', async ({ page }) => {
    const t = await setup(page, { signedIn: true, user: ADMIN })
    await mockUsers(page, 'fail')
    await page.goto('/dashboard/admin')
    await expect(page.getByRole('alert')).toBeVisible()
    await settled(page)
    await hide(page)

    await expect
      .poll(() =>
        t
          .islands()
          .filter((i) => i.name === 'AdminPanel')
          .map((i) => [i.stage, i.nav_type, i.page])
      )
      .toEqual([
        ['interactive', 'hard', '/dashboard/admin'],
        ['ready', 'hard', '/dashboard/admin'],
      ])
  })

  test('leaving before a panel is ready sends no ready mark, and no error', async ({ page }) => {
    const t = await setup(page, { signedIn: true, user: ADMIN })
    const users = await mockUsers(page, 'hold')
    await page.goto('/dashboard')
    await ready(page)
    await nav(page, 'Admin').click()
    await expect(page.getByTestId('admin-skeleton')).toBeVisible()
    await nav(page, 'Home').click()
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    // The list answers after the Member left.
    users.release()
    await page.evaluate(() => new Promise((r) => setTimeout(r, 200)))
    await settled(page)
    await hide(page)

    await expect.poll(() => t.navigations().length).toBe(3)
    const admin = t.islands().filter((i) => i.name === 'AdminPanel')
    expect(admin.map((i) => i.stage)).toEqual(['interactive'])
    expect(t.events()).toEqual([])
    expect(t.consoleErrors).toEqual([])
  })
})

test.describe('Navigation timing on a phone', () => {
  const { viewport, userAgent, deviceScaleFactor, isMobile, hasTouch } = devices['Pixel 7']
  test.use({ viewport, userAgent, deviceScaleFactor, isMobile, hasTouch })

  test('Home → Settings is reported from a mobile browser', async ({ page }) => {
    const t = await setup(page, { signedIn: true })
    await page.goto('/dashboard')
    await ready(page)
    await nav(page, 'Settings').tap()
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await settled(page)
    await hide(page)

    await expect
      .poll(() => t.navigations().map((n) => [n.page, n.nav_type]))
      .toContainEqual(['/dashboard/settings', 'soft'])
    // The server derives `device_class` from this header: "Mobile" → mobile.
    expect(t.userAgents.length).toBeGreaterThan(0)
    for (const ua of t.userAgents) expect(ua).toMatch(/Mobi/)
  })
})
