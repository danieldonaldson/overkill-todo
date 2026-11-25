import { devices, expect, test, type Page } from '@playwright/test'
import { hide, ready, setup, throwErrors } from './helpers/telemetry'

// Browser error reports (OT-006 slices 1 and 2, specs/observability and
// specs/dashboard). The API is mocked: /api/v1/client-telemetry records each
// beacon. Every document gets the production CSP from apps/web/Caddyfile, so a
// violation the reporter caused would show up here.
//
// Islands are crashed on purpose through TelemetryBoundary's test hook
// (`window.__otCrashIsland`), which `just e2e` builds in; release images don't.

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

  test('on a list address, an error holding the address sends no id', async ({ page }) => {
    const { rawBodies, reports } = await setup(page, { signedIn: true })
    const id = '3f2b8c1e-9d4a-4f6b-8c2d-1e0a9b8c7d6e'
    const list = { id, name: 'Weekend', open_count: 0, todo_count: 0 }
    await page.route('**/api/v1/lists', (route) => route.fulfill({ json: [list] }))
    await page.route('**/api/v1/lists/*', (route) => route.fulfill({ json: { list, todos: [] } }))

    await page.goto(`/dashboard/lists/${id}`)
    await expect(page.getByRole('heading', { name: 'Weekend' })).toBeVisible()
    await ready(page)
    await page.evaluate(() => {
      const href = location.href
      const error = new Error(`failed adding a todo at ${href}`)
      error.stack = `Error: failed at ${href}\n    at f (${href}:1:2)`
      setTimeout(() => {
        throw error
      })
    })
    await page.evaluate(() => new Promise((r) => setTimeout(r, 50)))
    await hide(page)

    await expect.poll(() => reports.length).toBeGreaterThan(0)
    expect(reports[0].page).toBe('/dashboard/lists/[id]')
    expect(rawBodies.join('\n')).not.toContain(id)
  })

  test('a failed add reports the route and status but never the title', async ({ page }) => {
    const { rawBodies, reports } = await setup(page, { signedIn: true })
    const id = '3f2b8c1e-9d4a-4f6b-8c2d-1e0a9b8c7d6e'
    const title = 'SECRET-TODO-TITLE'
    const list = { id, name: 'Weekend', open_count: 0, todo_count: 0, version: 1 }
    await page.route('**/api/v1/lists', (route) => route.fulfill({ json: [list] }))
    await page.route('**/api/v1/lists/*', (route) => route.fulfill({ json: { list, todos: [] } }))
    await page.route('**/api/v1/lists/*/todos', (route) =>
      route.fulfill({
        status: 500,
        json: { code: 'internal_error', message: 'Something went wrong' },
      })
    )

    await page.goto(`/dashboard/lists/${id}`)
    await page.getByLabel('New todo title').fill(title)
    await page.getByRole('button', { name: 'Add', exact: true }).click()
    await expect(page.getByText(/Couldn't save your change/)).toBeVisible()
    await hide(page)

    await expect.poll(() => reports.length).toBeGreaterThan(0)
    const failure = reports
      .flatMap((report) => report.events)
      .find((event) => event.api_route === '/lists/{id}/todos')
    expect(failure).toMatchObject({ status: 500 })
    expect(rawBodies.join('\n')).not.toContain(title)
  })

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
    // Two errors and the page load (sampled vitals and timings have their own
    // flows in web-vitals.spec.ts and navigation-timing.spec.ts).
    expect(
      reports[0].events
        .map((e) => e.type)
        .filter((t) => t === 'error' || t === 'page_load')
        .sort()
    ).toEqual(['error', 'error', 'page_load'])
  })

  test("a crashed panel's message fits the screen without horizontal scrolling", async ({
    page,
  }) => {
    const { events } = await setup(page, { signedIn: true, crash: 'SettingsPanel' })
    await page.goto('/dashboard/settings')
    const alert = page.getByRole('alert').filter({ hasText: "couldn't be shown" })
    await expect(alert).toBeVisible()
    await expect(alert.getByRole('button', { name: 'Try again' })).toBeInViewport()
    const box = (await alert.boundingBox())!
    const width = await page.evaluate(() => window.innerWidth)
    expect(box.x).toBeGreaterThanOrEqual(0)
    expect(box.x + box.width).toBeLessThanOrEqual(width)
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(
      width
    )
    await hide(page)
    await expect.poll(() => events().map((e) => e.island)).toEqual(['SettingsPanel'])
  })
})

/** The failure message a crashed island shows. */
const failure = (page: Page) => page.getByRole('alert').filter({ hasText: "couldn't be shown" })

test.describe('crashing panels are contained and reported', () => {
  test('a crashed Settings panel is replaced, and the header and navigation still work', async ({
    page,
  }) => {
    const { events, cspViolations } = await setup(page, { signedIn: true, crash: 'SettingsPanel' })
    await page.goto('/dashboard/settings')

    await expect(failure(page)).toBeVisible()
    await expect(failure(page).getByRole('button', { name: 'Try again' })).toBeVisible()
    await expect(page.getByRole('navigation', { name: /dashboard/i })).toBeVisible()

    await page.getByRole('link', { name: 'Home' }).click()
    await expect(page).toHaveURL('/dashboard')
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await expect(failure(page)).toHaveCount(0)

    await hide(page)
    await expect.poll(() => events().length).toBe(1)
    expect(events()[0]).toMatchObject({ kind: 'boundary', island: 'SettingsPanel' })
    expect(cspViolations).toEqual([])
  })

  test('a real rendering error from bad data is caught by the panel boundary', async ({ page }) => {
    // The slice-1 check's error: `oauth_providers` that isn't a list.
    const { events, reports } = await setup(page, {
      signedIn: true,
      user: { oauth_providers: 5 },
    })
    await page.goto('/dashboard/settings')
    await expect(failure(page)).toBeVisible()
    await hide(page)
    await expect
      .poll(() => events().map((e) => [e.kind, e.island]))
      .toEqual([['boundary', 'SettingsPanel']])
    expect(events()[0].message).toMatch(/includes is not a function/)
    expect(reports[0].page).toBe('/dashboard/settings')
  })

  test('Try again rebuilds the panel without a reload once the problem has gone', async ({
    page,
  }) => {
    await setup(page, { signedIn: true, crash: 'SettingsPanel' })
    await page.goto('/dashboard/settings')
    await expect(failure(page)).toBeVisible()
    await page.evaluate(() => {
      ;(window as unknown as { __marker: string }).__marker = 'same document'
      delete (window as unknown as { __otCrashIsland?: string }).__otCrashIsland
    })

    await failure(page).getByRole('button', { name: 'Try again' }).click()
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await expect(failure(page)).toHaveCount(0)
    expect(await page.evaluate(() => (window as unknown as { __marker?: string }).__marker)).toBe(
      'same document'
    )
  })

  test('Try again that fails again shows the message again and the page stays usable', async ({
    page,
  }) => {
    const { events } = await setup(page, { signedIn: true, crash: 'SettingsPanel' })
    await page.goto('/dashboard/settings')
    await failure(page).getByRole('button', { name: 'Try again' }).click()
    await expect(failure(page)).toBeVisible()
    await page.getByRole('link', { name: 'Home' }).click()
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await hide(page)
    // Both failures, as one event with a count.
    await expect
      .poll(() => events().map((e) => [e.island, e.count]))
      .toEqual([['SettingsPanel', 2]])
  })

  test('a crashed header shows the message in its place and the page stays usable', async ({
    page,
  }) => {
    const { events } = await setup(page, { signedIn: true, crash: 'DashboardHeader' })
    await page.goto('/dashboard')
    await expect(failure(page)).toBeVisible()
    await expect(page.getByRole('navigation', { name: /dashboard/i })).toHaveCount(0)
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await hide(page)
    await expect.poll(() => events().map((e) => e.island)).toEqual(['DashboardHeader'])
  })

  test('a crashed sign-in form offers Try again and a reload', async ({ page }) => {
    const { events } = await setup(page, { crash: 'AuthScreen' })
    await page.goto('/login')
    await expect(failure(page)).toBeVisible()
    await expect(failure(page).getByRole('button', { name: 'Try again' })).toBeVisible()
    await expect(failure(page).getByRole('button', { name: 'Reload the page' })).toBeVisible()
    const text = (await failure(page).textContent()) ?? ''
    expect(text).not.toMatch(/Deliberate|crash|Error|trace/i)
    await hide(page)
    await expect
      .poll(() => events().map((e) => [e.kind, e.island]))
      .toEqual([['boundary', 'AuthScreen']])
  })
})

test.describe('failed API calls', () => {
  const TRACE = 'abcdef0123456789abcdef0123456789'

  test('a server error during a dashboard action is reported with route, status and trace ID', async ({
    page,
  }) => {
    const { events } = await setup(page, { signedIn: true })
    await page.route('**/api/v1/users/password/change', (route) =>
      route.fulfill({
        status: 500,
        headers: { 'x-trace-id': TRACE },
        json: { code: 'internal_error', message: 'Internal server error' },
      })
    )
    await page.goto('/dashboard/settings')
    await page.getByLabel('Current password').fill('old password 1234')
    await page.getByLabel('New password').fill('Tr0ub4dor&3-horse-battery')
    await page.getByRole('button', { name: 'Change password' }).click()
    await expect(page.getByText(/Internal server error/)).toBeVisible()
    await hide(page)

    await expect.poll(() => events().length).toBe(1)
    expect(events()[0]).toMatchObject({
      kind: 'api',
      api_route: '/users/password/change',
      status: 500,
      trace_id: TRACE,
      count: 1,
    })
  })

  test('an unreachable server is reported once it answers again', async ({ page }) => {
    const { events } = await setup(page)
    let down = true
    await page.route('**/api/v1/auth/register', (route) =>
      down
        ? route.abort('connectionrefused')
        : route.fulfill({ json: { message: 'Check your email' } })
    )
    await page.goto('/register')
    await ready(page)
    await page.getByLabel('Email').fill('visitor@example.com')
    await page.getByRole('button', { name: /continue/i }).click()
    await expect(page.getByRole('alert')).toBeVisible()

    // Held while the server is unreachable, even past the 10 s timer.
    await page.clock.install()
    await page.clock.fastForward(30_000)
    expect(events()).toEqual([])

    down = false
    await page.getByRole('button', { name: /continue/i }).click()
    await expect.poll(() => events().length).toBe(1)
    expect(events()[0]).toMatchObject({
      kind: 'api',
      api_route: '/auth/register',
      network_failed: true,
    })
    expect(events()[0]).not.toHaveProperty('status')
    expect(events()[0]).not.toHaveProperty('trace_id')
  })

  test('an unreachable server is reported when the page is next hidden', async ({ page }) => {
    const { events } = await setup(page)
    await page.route('**/api/v1/auth/register', (route) => route.abort('connectionrefused'))
    await page.goto('/register')
    await ready(page)
    await page.getByLabel('Email').fill('visitor@example.com')
    await page.getByRole('button', { name: /continue/i }).click()
    await expect(page.getByRole('alert')).toBeVisible()
    await hide(page)
    await expect.poll(() => events().map((e) => e.network_failed)).toEqual([true])
  })

  test('a wrong password (401) is not a browser error', async ({ page }) => {
    const { events, pageLoads } = await setup(page)
    await page.route('**/api/v1/auth/login', (route) =>
      route.fulfill({
        status: 401,
        json: { code: 'unauthorized', message: 'Invalid email or password' },
      })
    )
    await page.goto('/login')
    await ready(page)
    await page.getByLabel(/email/i).fill('member@example.com')
    await page.getByRole('textbox', { name: /password/i }).fill('wrong password')
    await page.getByRole('button', { name: /^sign in$/i }).click()
    await expect(page.getByText(/invalid email or password/i)).toBeVisible()
    await hide(page)
    await expect.poll(() => pageLoads().length).toBe(1)
    expect(events()).toEqual([])
  })
})

test.describe('page loads', () => {
  test('the full load and each in-site navigation are counted once, in one page load', async ({
    page,
  }) => {
    const { pageLoads } = await setup(page, { signedIn: true })
    await page.goto('/dashboard')
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    // Before the router is ready a click is a full load: a new document.
    await ready(page)
    await page.getByRole('link', { name: 'Settings' }).click()
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await hide(page)

    await expect
      .poll(() => pageLoads().map(([p]) => p))
      .toEqual(['/dashboard', '/dashboard/settings'])
    expect(new Set(pageLoads().map(([, id]) => id)).size).toBe(1)
  })
})
