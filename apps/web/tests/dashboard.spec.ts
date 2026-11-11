import { expect, test, type Page } from '@playwright/test'
import { seedSession, TEST_USER } from './helpers/session'

test.describe('dashboard static shells', () => {
  test.beforeEach(async ({ page }) => {
    await seedSession(page)
    await page.route('**/api/v1/auth/me', async (route) => {
      await route.fulfill({
        json: {
          id: 'test-user',
          email: 'test@example.com',
          first_name: '',
          last_name: '',
          email_verified: true,
          has_password: true,
          oauth_providers: [],
        },
      })
    })
  })

  test('dashboard route serves protected shell', async ({ page }) => {
    await page.goto('/dashboard')

    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await expect(page.getByRole('navigation', { name: /dashboard/i })).toBeVisible()
  })

  test('dashboard navigation does not reload the document', async ({ page }) => {
    await page.goto('/dashboard')

    await page.evaluate(() => {
      ;(window as Window & { __dashboardNavigationMarker?: string }).__dashboardNavigationMarker =
        'preserved'
    })

    await page.getByRole('link', { name: 'Settings' }).click()

    await expect(page).toHaveURL('/dashboard/settings')
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (window as Window & { __dashboardNavigationMarker?: string })
              .__dashboardNavigationMarker
        )
      )
      .toBe('preserved')
  })

  test('settings route serves protected shell', async ({ page }) => {
    await page.goto('/dashboard/settings')

    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await expect(page.getByText('Manage your dashboard account.')).toBeVisible()
  })

  // The header's background check fetches the user once and the panels read
  // it from the session store: one /auth/me per page load, not one per island.
  for (const { path, heading, role } of [
    { path: '/dashboard', heading: 'Home', role: 'user' },
    { path: '/dashboard/settings', heading: 'Settings', role: 'user' },
    { path: '/dashboard/admin', heading: 'Admin', role: 'admin' },
  ]) {
    test(`${path} calls /auth/me exactly once`, async ({ page }) => {
      let meCount = 0
      await page.route('**/api/v1/auth/me', async (route) => {
        meCount += 1
        await route.fulfill({
          json: {
            id: 'test-user',
            email: 'test@example.com',
            first_name: '',
            last_name: '',
            email_verified: true,
            has_password: true,
            oauth_providers: [],
            role,
          },
        })
      })
      await page.route('**/api/v1/admin/users*', async (route) => {
        await route.fulfill({ json: { users: [], limit: 50, offset: 0 } })
      })

      await page.goto(path)

      await expect(page.getByRole('heading', { name: heading, exact: true })).toBeVisible()
      await page.waitForLoadState('networkidle')
      expect(meCount).toBe(1)
    })
  }
})

// Records whether a full-screen "Loading..." ever reached the DOM of the
// current document. Installed before any page script, so it sees parser-inserted
// nodes as well as island renders.
async function watchForLoadingText(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as Window & { __sawLoading?: boolean }
    w.__sawLoading = false
    const check = (node: Node) => {
      if (node.textContent?.includes('Loading...')) w.__sawLoading = true
    }
    new MutationObserver((records) => {
      for (const record of records) {
        if (record.type === 'characterData') check(record.target)
        record.addedNodes.forEach(check)
      }
    }).observe(document, { childList: true, subtree: true, characterData: true })
  })
}

function sawLoading(page: Page): Promise<boolean | undefined> {
  return page.evaluate(() => (window as Window & { __sawLoading?: boolean }).__sawLoading)
}

// OT-003 hypothesis: the dashboard is optimistic. With a session hint it shows
// at once, confirms the account in the background (one /auth/me per document),
// and keeps its header in place across client-side moves.
test.describe('instant dashboard', () => {
  test('reloading Settings with a cached user shows the email before /auth/me answers, with no spinner', async ({
    page,
  }) => {
    const user = { ...TEST_USER, role: 'user' }
    await seedSession(page, { cachedUser: user })
    await watchForLoadingText(page)

    // /auth/me answers only once the test releases it.
    let release!: () => void
    const released = new Promise<void>((resolve) => (release = resolve))
    let holdMe = false
    let meAnswered = 0
    await page.route('**/api/v1/auth/me', async (route) => {
      if (holdMe) await released
      await route.fulfill({ json: user })
      meAnswered += 1
    })

    await page.goto('/dashboard/settings')
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await page.waitForLoadState('networkidle')

    holdMe = true
    const answeredBeforeReload = meAnswered
    await page.reload()

    await expect(page.getByRole('main').getByText(user.email)).toBeVisible()
    expect(meAnswered, '/auth/me has not answered yet').toBe(answeredBeforeReload)
    expect(await sawLoading(page), 'no "Loading..." was ever rendered').toBe(false)

    release()
  })

  test('moving Home → Settings → Home keeps the header, updates aria-current, and checks the account once', async ({
    page,
  }) => {
    await seedSession(page)
    let meCount = 0
    await page.route('**/api/v1/auth/me', async (route) => {
      meCount += 1
      await route.fulfill({ json: { ...TEST_USER, role: 'user' } })
    })

    await page.goto('/dashboard')
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()

    const header = await page.evaluateHandle(() => {
      const el = document.querySelector('header') as HTMLElement & { __e2eTag?: string }
      el.__e2eTag = 'kept'
      return el
    })
    const sameHeader = () =>
      header.evaluate(
        (el: HTMLElement & { __e2eTag?: string }) =>
          el.isConnected && el === document.querySelector('header') && el.__e2eTag === 'kept'
      )

    const nav = page.getByRole('navigation', { name: /dashboard/i })
    const homeLink = nav.getByRole('link', { name: 'Home', exact: true })
    const settingsLink = nav.getByRole('link', { name: 'Settings', exact: true })

    await settingsLink.click()
    await expect(page).toHaveURL('/dashboard/settings')
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await expect(settingsLink).toHaveAttribute('aria-current', 'page')
    await expect(homeLink).not.toHaveAttribute('aria-current', 'page')
    expect(await sameHeader(), 'header survives Home → Settings').toBe(true)

    await homeLink.click()
    await expect(page).toHaveURL('/dashboard')
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await expect(homeLink).toHaveAttribute('aria-current', 'page')
    await expect(settingsLink).not.toHaveAttribute('aria-current', 'page')
    expect(await sameHeader(), 'header survives Settings → Home').toBe(true)

    await page.waitForLoadState('networkidle')
    expect(meCount).toBe(1)
  })

  test('the site root opens the dashboard for a signed-in browser, never showing sign-in', async ({
    page,
  }) => {
    await seedSession(page, { cachedUser: TEST_USER })
    await page.route('**/api/v1/auth/me', (route) => route.fulfill({ json: TEST_USER }))
    // Records, in sessionStorage so it survives navigations, whether any
    // document on the way was the sign-in page.
    await page.addInitScript(() => {
      if (location.pathname.startsWith('/login')) sessionStorage.setItem('__sawLogin', '1')
    })

    await page.goto('/')

    await expect(page).toHaveURL(/\/dashboard\/?$/)
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    expect(await page.evaluate(() => sessionStorage.getItem('__sawLogin'))).toBeNull()
  })

  test('without a session hint, /dashboard goes to /login without showing the dashboard', async ({
    page,
  }) => {
    let meCount = 0
    await page.route('**/api/v1/auth/me', async (route) => {
      meCount += 1
      await route.fulfill({ status: 401, json: { message: 'unauthenticated' } })
    })
    // Records, in sessionStorage so it survives the redirect, any dashboard
    // heading that became visible on a /dashboard document.
    await page.addInitScript(() => {
      if (!location.pathname.startsWith('/dashboard')) return
      const record = () => {
        for (const h of document.querySelectorAll('h1, h2')) {
          if (h.checkVisibility())
            sessionStorage.setItem('__sawDashboardHeading', h.textContent ?? '')
        }
      }
      new MutationObserver(record).observe(document, { childList: true, subtree: true })
    })

    await page.goto('/dashboard')

    await expect(page).toHaveURL(/\/login\/?$/)
    await expect(page.getByRole('heading').first()).toBeVisible()
    expect(
      await page.evaluate(() => sessionStorage.getItem('__sawDashboardHeading')),
      'no dashboard heading was ever visible'
    ).toBeNull()
    expect(meCount, 'no account check was made').toBe(0)
  })
})
