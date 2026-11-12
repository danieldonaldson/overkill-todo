import { expect, test, type Page, type Route } from '@playwright/test'
import { seedSession, TEST_USER, TOKEN_EXPIRY_KEY } from './helpers/session'

// OT-003 wrong-guess and account flows for the optimistic dashboard, with the
// API mocked. The happy path (no spinner, header kept, one /auth/me) and the
// no-hint redirect are in dashboard.spec.ts.

const MEMBER = { ...TEST_USER, role: 'user' }
const ADMIN = { ...TEST_USER, role: 'admin' }

/** A promise the test resolves by hand, for holding a mocked response. */
function gate(): { opened: Promise<void>; open: () => void } {
  let open!: () => void
  const opened = new Promise<void>((resolve) => (open = resolve))
  return { opened, open }
}

/** Fulfils, ignoring a page that has already navigated away from the request. */
async function fulfill(route: Route, options: Parameters<Route['fulfill']>[0]): Promise<void> {
  await route.fulfill(options).catch(() => {})
}

/** Mocks /auth/me with `user`, counting calls. */
async function mockMe(page: Page, user: object): Promise<{ count: () => number }> {
  let count = 0
  await page.route('**/api/v1/auth/me', async (route) => {
    count += 1
    await fulfill(route, { json: user })
  })
  return { count: () => count }
}

/**
 * A readable CSRF cookie, as a signed-in browser has: mutating requests then go
 * straight out instead of refreshing first to mint one.
 */
async function seedCsrfCookie(page: Page, baseURL: string | undefined): Promise<void> {
  await page.context().addCookies([{ name: 'csrf_token', value: 'e2e-csrf', url: baseURL! }])
}

/**
 * Records in sessionStorage (so it survives redirects and reloads in the tab)
 * the path of the first document on which a visible element matching
 * `selector`, or visible text containing `text`, appeared.
 */
async function recordIfEverShown(
  page: Page,
  key: string,
  match: { selector?: string; text?: string }
): Promise<void> {
  await page.addInitScript(
    ({ key, selector, text }) => {
      const check = () => {
        try {
          if (sessionStorage.getItem(key)) return
          const seen =
            (selector !== undefined &&
              [...document.querySelectorAll(selector)].some((el) => el.checkVisibility())) ||
            (text !== undefined && (document.body?.innerText.includes(text) ?? false))
          if (seen) sessionStorage.setItem(key, location.pathname)
        } catch {
          // Opaque origins have no storage.
        }
      }
      new MutationObserver(check).observe(document, {
        childList: true,
        subtree: true,
        characterData: true,
      })
    },
    { key, selector: match.selector, text: match.text }
  )
}

function recorded(page: Page, key: string): Promise<string | null> {
  return page.evaluate((key) => sessionStorage.getItem(key), key)
}

/** Sets a window property that only survives client-side moves, not reloads. */
async function markDocument(page: Page): Promise<() => Promise<boolean>> {
  await page.evaluate(() => {
    ;(window as Window & { __e2eDocument?: string }).__e2eDocument = 'kept'
  })
  return () =>
    page.evaluate(() => (window as Window & { __e2eDocument?: string }).__e2eDocument === 'kept')
}

/** Tags the current header element; the returned check is true while it is the same node. */
async function tagHeader(page: Page): Promise<() => Promise<boolean>> {
  const header = await page.evaluateHandle(() => {
    const el = document.querySelector('header') as HTMLElement & { __e2eTag?: string }
    el.__e2eTag = 'kept'
    return el
  })
  return () =>
    header.evaluate(
      (el: HTMLElement & { __e2eTag?: string }) =>
        el.isConnected && el === document.querySelector('header') && el.__e2eTag === 'kept'
    )
}

function dashboardNav(page: Page) {
  const nav = page.getByRole('navigation', { name: /dashboard/i })
  return {
    nav,
    home: nav.getByRole('link', { name: 'Home', exact: true }),
    settings: nav.getByRole('link', { name: 'Settings', exact: true }),
    admin: nav.getByRole('link', { name: 'Admin', exact: true }),
  }
}

/** Settings → Home → Settings through the dashboard navigation (client-side). */
async function roundTripViaHome(page: Page): Promise<void> {
  const { home, settings } = dashboardNav(page)
  await home.click()
  await expect(page).toHaveURL('/dashboard')
  await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
  await settings.click()
  await expect(page).toHaveURL('/dashboard/settings')
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
}

test.describe('wrong guesses', () => {
  test('a session the server refuses lands on /login?session=expired with no account data shown', async ({
    page,
  }) => {
    await seedSession(page)
    await recordIfEverShown(page, '__sawEmail', { text: MEMBER.email })
    let refreshCount = 0
    await page.route('**/api/v1/auth/refresh', async (route) => {
      refreshCount += 1
      await fulfill(route, { status: 401, json: { message: 'refresh expired' } })
    })
    await page.route('**/api/v1/auth/me', async (route) => {
      await fulfill(route, { status: 401, json: { message: 'token expired' } })
    })

    await page.goto('/dashboard')

    await expect(page).toHaveURL(/\/login\?session=expired$/)
    await expect(page.getByText('Your session has expired. Please sign in again.')).toBeVisible()
    await expect(page.getByRole('navigation', { name: /dashboard/i })).toHaveCount(0)
    expect(refreshCount, 'one refresh was tried').toBe(1)
    expect(await recorded(page, '__sawEmail'), 'the account email was never shown').toBeNull()
    expect(
      await page.evaluate((key) => localStorage.getItem(key), TOKEN_EXPIRY_KEY),
      'the session hint is cleared'
    ).toBeNull()
  })

  test('an unverified account is sent to /verify-email once the check answers', async ({
    page,
  }) => {
    await seedSession(page)
    await mockMe(page, { ...MEMBER, email_verified: false })

    await page.goto('/dashboard')

    await expect(page).toHaveURL(`/verify-email?email=${encodeURIComponent(MEMBER.email)}`)
    await expect(page.getByRole('heading', { name: 'Verify your email' })).toBeVisible()
  })

  test('an unreachable server keeps the page with remembered details, and Retry clears the notice', async ({
    page,
  }) => {
    await seedSession(page, { cachedUser: MEMBER })
    let meCount = 0
    let serverDown = true
    await page.route('**/api/v1/auth/me', async (route) => {
      meCount += 1
      if (serverDown) {
        await fulfill(route, { status: 500, json: { code: 'internal', message: 'boom' } })
        return
      }
      await fulfill(route, { json: MEMBER })
    })

    await page.goto('/dashboard')

    const notice = page.getByRole('status').filter({ hasText: "Couldn't reach the server" })
    await expect(notice).toBeVisible()
    await expect(page).toHaveURL('/dashboard')
    await expect(page.getByText(`Signed in as ${MEMBER.email}`)).toBeVisible()
    const sameDocument = await markDocument(page)

    serverDown = false
    await notice.getByRole('button', { name: 'Retry' }).click()

    await expect(notice).toHaveCount(0)
    expect(meCount, 'Retry repeated the check').toBe(2)
    await expect(page).toHaveURL('/dashboard')
    await expect(page.getByText(`Signed in as ${MEMBER.email}`)).toBeVisible()
    expect(await sameDocument(), 'no reload').toBe(true)
  })

  test('a new tab shows placeholders for account details, then the email once the check answers', async ({
    page,
  }) => {
    await seedSession(page) // hint, but nothing cached in this tab
    const answer = gate()
    await page.route('**/api/v1/auth/me', async (route) => {
      await answer.opened
      await fulfill(route, { json: MEMBER })
    })

    await page.goto('/dashboard/settings')

    // Header and page layout at once; account sections are placeholders.
    await expect(page.getByRole('navigation', { name: /dashboard/i })).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await expect(page.getByTestId('settings-skeleton')).toBeVisible()
    await expect(page.getByText(MEMBER.email)).toHaveCount(0)
    await expect(page.getByRole('button', { name: 'Log out' })).toHaveCount(0)

    answer.open()

    await expect(page.getByRole('main').getByText(MEMBER.email)).toBeVisible()
    await expect(page.getByTestId('settings-skeleton')).toHaveCount(0)
    await expect(page.getByRole('button', { name: 'Log out' })).toBeVisible()
  })

  test('a stale cached account is corrected by the check without a reload', async ({ page }) => {
    const stale = { ...MEMBER, email: 'old@example.com', has_password: false }
    const current = { ...MEMBER, email: 'new@example.com', has_password: true }
    await seedSession(page, { cachedUser: stale })
    const answer = gate()
    await page.route('**/api/v1/auth/me', async (route) => {
      await answer.opened
      await fulfill(route, { json: current })
    })

    await page.goto('/dashboard/settings')

    const main = page.getByRole('main')
    await expect(main.getByText(stale.email)).toBeVisible()
    await expect(main.getByText('Set a password')).toBeVisible()
    const sameDocument = await markDocument(page)

    answer.open()

    await expect(main.getByText(current.email)).toBeVisible()
    await expect(main.getByText(stale.email)).toHaveCount(0)
    await expect(main.getByRole('button', { name: 'Change password' })).toBeVisible()
    await expect(main.getByText('Set a password')).toHaveCount(0)
    expect(await sameDocument(), 'no reload').toBe(true)
  })
})

// Each action patches the tab's one record of the account. /auth/me keeps
// answering with the original account and is called once, so what Settings
// shows after the move comes from the record, not from a fresh check.
test.describe('account changes persist across moves', () => {
  test('setting a password is remembered after moving to Home and back', async ({
    page,
    baseURL,
  }) => {
    await seedSession(page)
    await seedCsrfCookie(page, baseURL)
    const me = await mockMe(page, { ...MEMBER, has_password: false })
    let setBody: unknown = null
    await page.route('**/api/v1/users/password/set', async (route) => {
      setBody = route.request().postDataJSON()
      await fulfill(route, { status: 204 })
    })

    await page.goto('/dashboard/settings')
    const main = page.getByRole('main')
    await expect(main.getByText('Set a password')).toBeVisible()
    const sameDocument = await markDocument(page)

    await page.getByRole('textbox', { name: /^New password/ }).fill('a-new-password')
    await main.getByRole('button', { name: 'Set password', exact: true }).click()
    await expect(page.getByText('Password set.')).toBeVisible()
    expect(setBody).toEqual({ new_password: 'a-new-password' })

    await roundTripViaHome(page)

    await expect(main.getByRole('button', { name: 'Change password' })).toBeVisible()
    await expect(page.getByRole('textbox', { name: /^Current password/ })).toBeVisible()
    await expect(main.getByText('Set a password')).toHaveCount(0)
    expect(await sameDocument(), 'no reload').toBe(true)
    expect(me.count()).toBe(1)
  })

  test('unlinking Google is remembered after moving to Home and back', async ({
    page,
    baseURL,
  }) => {
    await seedSession(page)
    await seedCsrfCookie(page, baseURL)
    const me = await mockMe(page, { ...MEMBER, oauth_providers: ['google'] })
    let unlinkCount = 0
    await page.route('**/api/v1/auth/oauth/google/unlink', async (route) => {
      unlinkCount += 1
      await fulfill(route, { status: 204 })
    })

    await page.goto('/dashboard/settings')
    const main = page.getByRole('main')
    await expect(main.getByText('Linked', { exact: true })).toBeVisible()
    const sameDocument = await markDocument(page)

    await main.getByRole('button', { name: 'Unlink Google' }).click()
    await expect(page.getByText('Google account unlinked.')).toBeVisible()
    expect(unlinkCount).toBe(1)

    await roundTripViaHome(page)

    await expect(main.getByText('Not linked', { exact: true })).toBeVisible()
    await expect(main.getByRole('button', { name: 'Link Google' })).toBeVisible()
    await expect(main.getByRole('button', { name: 'Unlink Google' })).toHaveCount(0)
    expect(await sameDocument(), 'no reload').toBe(true)
    expect(me.count()).toBe(1)
  })

  test('scheduled deletion is remembered after moving to Home and back', async ({
    page,
    baseURL,
  }) => {
    await seedSession(page)
    await seedCsrfCookie(page, baseURL)
    const me = await mockMe(page, MEMBER)
    await page.route('**/api/v1/users/me', async (route) => {
      if (route.request().method() !== 'DELETE') {
        await route.fallback()
        return
      }
      await fulfill(route, { json: { deletion_scheduled_at: '2026-07-04T00:00:00Z' } })
    })

    await page.goto('/dashboard/settings')
    const main = page.getByRole('main')
    await expect(main.getByRole('button', { name: 'Delete my account' })).toBeVisible()
    const sameDocument = await markDocument(page)

    await page.getByLabel('Confirm your password').fill('hunter2')
    await page.getByLabel('Type DELETE to confirm').fill('DELETE')
    await main.getByRole('button', { name: 'Delete my account' }).click()
    await expect(main.getByText(/scheduled for deletion on/)).toBeVisible()

    await roundTripViaHome(page)

    await expect(main.getByText(/scheduled for deletion on/)).toBeVisible()
    await expect(main.getByRole('button', { name: 'Cancel deletion' })).toBeVisible()
    await expect(page.getByLabel('Type DELETE to confirm')).toHaveCount(0)
    expect(await sameDocument(), 'no reload').toBe(true)
    expect(me.count()).toBe(1)
  })

  test('a failed unlink leaves Google linked, also after moving to Home and back', async ({
    page,
    baseURL,
  }) => {
    await seedSession(page)
    await seedCsrfCookie(page, baseURL)
    await mockMe(page, { ...MEMBER, oauth_providers: ['google'] })
    await page.route('**/api/v1/auth/oauth/google/unlink', async (route) => {
      await fulfill(route, {
        status: 409,
        json: { code: 'conflict', message: 'Set a password before unlinking Google' },
      })
    })

    await page.goto('/dashboard/settings')
    const main = page.getByRole('main')
    await expect(main.getByText('Linked', { exact: true })).toBeVisible()

    await main.getByRole('button', { name: 'Unlink Google' }).click()
    await expect(page.getByRole('alert')).toContainText('Set a password before unlinking Google')
    await expect(main.getByText('Linked', { exact: true })).toBeVisible()

    await roundTripViaHome(page)

    await expect(main.getByText('Linked', { exact: true })).toBeVisible()
    await expect(main.getByRole('button', { name: 'Unlink Google' })).toBeVisible()
    await expect(main.getByText('Not linked', { exact: true })).toHaveCount(0)
  })
})

test.describe('sign-out', () => {
  test('Log out on Settings lands on /login and asks the server to end the session', async ({
    page,
    baseURL,
  }) => {
    await seedSession(page, { cachedUser: MEMBER })
    await seedCsrfCookie(page, baseURL)
    await mockMe(page, MEMBER)
    let logoutCount = 0
    await page.route('**/api/v1/auth/logout', async (route) => {
      logoutCount += 1
      await fulfill(route, { status: 204 })
    })

    await page.goto('/dashboard/settings')
    await page.getByRole('button', { name: 'Log out' }).click()

    await expect(page).toHaveURL(/\/login\/?$/)
    await expect(page.getByLabel('Email')).toBeVisible()
    expect(logoutCount).toBe(1)
    expect(await page.evaluate((key) => localStorage.getItem(key), TOKEN_EXPIRY_KEY)).toBeNull()
  })

  test('pressing back after signing out shows no account content and ends on /login', async ({
    page,
    baseURL,
  }) => {
    await seedSession(page, { cachedUser: MEMBER })
    await seedCsrfCookie(page, baseURL)
    await mockMe(page, MEMBER)
    await page.route('**/api/v1/auth/logout', async (route) => {
      await fulfill(route, { status: 204 })
    })
    // From sign-out on, record any dashboard document (fresh or restored from
    // the back/forward cache) and any sight of the account's email.
    await page.addInitScript((email) => {
      const signedOut = () => sessionStorage.getItem('__signedOut') === '1'
      const note = (what: string) => {
        if (signedOut() && !sessionStorage.getItem('__afterSignOut'))
          sessionStorage.setItem('__afterSignOut', `${what} on ${location.pathname}`)
      }
      addEventListener('pageshow', (event) => {
        if (event.persisted && location.pathname.startsWith('/dashboard'))
          note('restored dashboard')
      })
      new MutationObserver(() => {
        if (document.body?.innerText.includes(email)) note('email shown')
        if (document.querySelector('nav[aria-label="Dashboard"]')?.checkVisibility())
          note('dashboard nav shown')
      }).observe(document, { childList: true, subtree: true, characterData: true })
    }, MEMBER.email)

    await page.goto('/dashboard')
    await expect(page.getByText(`Signed in as ${MEMBER.email}`)).toBeVisible()
    await dashboardNav(page).settings.click()
    await expect(page).toHaveURL('/dashboard/settings')

    await page.getByRole('button', { name: 'Log out' }).click()
    await expect(page).toHaveURL(/\/login\/?$/)
    await page.evaluate(() => sessionStorage.setItem('__signedOut', '1'))

    // The Settings entry was replaced by /login; back goes to the Home entry.
    await page.goBack({ waitUntil: 'commit' })

    await expect(page).toHaveURL(/\/login\/?$/)
    await expect(page.getByLabel('Email')).toBeVisible()
    await expect(page.getByText(MEMBER.email)).toHaveCount(0)
    expect(await recorded(page, '__afterSignOut'), 'nothing of the account after sign-out').toBe(
      null
    )
  })

  test('with the server not answering, sign-out still lands on /login within a second', async ({
    page,
    baseURL,
  }) => {
    await seedSession(page, { cachedUser: MEMBER })
    await seedCsrfCookie(page, baseURL)
    await mockMe(page, MEMBER)
    // Never answered while the test runs: the server hangs. Aborted at the
    // end so the request doesn't fall through to the preview proxy.
    const hung: Route[] = []
    await page.route('**/api/v1/auth/logout', (route) => {
      hung.push(route)
    })

    await page.goto('/dashboard/settings')
    const logOut = page.getByRole('button', { name: 'Log out' })
    await expect(logOut).toBeVisible()

    const started = Date.now()
    await logOut.click()
    await page.waitForURL(/\/login\/?$/)
    const elapsed = Date.now() - started

    expect(hung, 'logout was sent').toHaveLength(1)
    // 1 s bound plus slack for the click, the redirect and CI noise.
    expect(elapsed).toBeLessThan(2500)

    // Opening a dashboard page afterwards goes to sign-in.
    await page.goto('/dashboard')
    await expect(page).toHaveURL(/\/login\/?$/)
    await expect(page.getByRole('navigation', { name: /dashboard/i })).toHaveCount(0)

    await Promise.all(hung.map((route) => route.abort().catch(() => {})))
  })
})

test.describe('admin-only pages', () => {
  test('a Member never sees the Admin entry, including before the check answers', async ({
    page,
  }) => {
    await seedSession(page) // nothing cached: the role is unknown at first
    await recordIfEverShown(page, '__sawAdminEntry', { selector: 'a[href="/dashboard/admin"]' })
    const answer = gate()
    let meCount = 0
    await page.route('**/api/v1/auth/me', async (route) => {
      meCount += 1
      await answer.opened
      await fulfill(route, { json: MEMBER })
    })

    await page.goto('/dashboard')
    const { home, settings, admin } = dashboardNav(page)
    await expect(home).toBeVisible()
    await expect(settings).toBeVisible()
    await expect.poll(() => meCount, '/auth/me is in flight').toBe(1)
    await expect(admin).toHaveCount(0)

    answer.open()
    await expect(page.getByText(`Signed in as ${MEMBER.email}`)).toBeVisible()
    await expect(admin).toHaveCount(0)

    await settings.click()
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await expect(admin).toHaveCount(0)
    await home.click()
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await expect(admin).toHaveCount(0)

    expect(await recorded(page, '__sawAdminEntry'), 'the Admin entry never appeared').toBeNull()
  })

  test('a Member opening /dashboard/admin is sent Home with no user list shown', async ({
    page,
  }) => {
    await seedSession(page)
    await recordIfEverShown(page, '__sawUserList', { selector: 'table' })
    await recordIfEverShown(page, '__sawUsersCard', { text: 'All registered users.' })
    // Hold the admin page's /auth/me, so the 403 from the list is what sends
    // the Member Home; later documents get their answer at once.
    const firstCheck = gate()
    let meCount = 0
    await page.route('**/api/v1/auth/me', async (route) => {
      meCount += 1
      if (meCount === 1) await firstCheck.opened
      await fulfill(route, { json: MEMBER })
    })
    let listCount = 0
    await page.route('**/api/v1/admin/users*', async (route) => {
      listCount += 1
      await fulfill(route, { status: 403, json: { code: 'forbidden', message: 'Forbidden' } })
    })

    await page.goto('/dashboard/admin')

    await expect(page).toHaveURL('/dashboard')
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    firstCheck.open()
    await expect(page.getByText(`Signed in as ${MEMBER.email}`)).toBeVisible()
    expect(listCount, 'the list was requested once, from the Admin page').toBe(1)
    expect(await recorded(page, '__sawUserList'), 'no user table was ever shown').toBeNull()
    expect(await recorded(page, '__sawUsersCard'), 'no user list card was ever shown').toBeNull()
  })

  test('an Admin moves Home → Admin via the nav, keeping the header, and sees the list', async ({
    page,
  }) => {
    await seedSession(page) // nothing cached: the entry appears once the check answers
    const me = await mockMe(page, ADMIN)
    await page.route('**/api/v1/admin/users*', async (route) => {
      await fulfill(route, {
        json: {
          users: [
            {
              id: 'other-user',
              email: 'other@example.com',
              role: 'user',
              created_at: '2026-01-01T00:00:00Z',
            },
          ],
          total: 1,
          limit: 50,
          offset: 0,
        },
      })
    })

    await page.goto('/dashboard')
    const { home, admin } = dashboardNav(page)
    await expect(admin).toBeVisible()
    const sameHeader = await tagHeader(page)

    await admin.click()

    await expect(page).toHaveURL('/dashboard/admin')
    await expect(page.getByRole('heading', { name: 'Admin' })).toBeVisible()
    await expect(page.getByRole('cell', { name: 'other@example.com' })).toBeVisible()
    await expect(admin).toHaveAttribute('aria-current', 'page')
    await expect(home).not.toHaveAttribute('aria-current', 'page')
    expect(await sameHeader(), 'header survives Home → Admin').toBe(true)
    expect(me.count()).toBe(1)
  })
})

test.describe('phone width', () => {
  test.use({ viewport: { width: 390, height: 844 } })

  test('the compact nav stays in place across a move and marks the new page', async ({ page }) => {
    await seedSession(page, { cachedUser: MEMBER })
    await mockMe(page, MEMBER)

    await page.goto('/dashboard')
    const { nav, home, settings } = dashboardNav(page)
    await expect(home).toHaveAttribute('aria-current', 'page')
    // Compact: icon-only links (labels are screen-reader only), within the screen.
    const settingsBox = await settings.boundingBox()
    expect(settingsBox!.width).toBeLessThan(60)
    const navBox = await nav.boundingBox()
    expect(navBox!.x + navBox!.width).toBeLessThanOrEqual(390)
    const sameHeader = await tagHeader(page)

    await settings.click()

    await expect(page).toHaveURL('/dashboard/settings')
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await expect(settings).toHaveAttribute('aria-current', 'page')
    await expect(home).not.toHaveAttribute('aria-current', 'page')
    expect((await settings.boundingBox())!.width).toBeLessThan(60)
    expect(await sameHeader(), 'header survives the move').toBe(true)
  })
})

// The head script that sends a browser with no session hint to /login runs on
// full loads. On a ClientRouter swap Astro may or may not run it (today it
// does, as a script it hasn't seen), so the header's session.init() repeats the
// check as a backstop. Both tests start on /login with no hint and follow an
// in-page link the ClientRouter intercepts.
test.describe('client-side navigation into the dashboard', () => {
  async function followLinkToDashboard(page: Page): Promise<{ meCount: () => number }> {
    let meCount = 0
    await page.route('**/api/v1/auth/me', async (route) => {
      meCount += 1
      await fulfill(route, { status: 401, json: { message: 'unauthenticated' } })
    })
    // Records the client-side swap into /dashboard, and whether the login
    // document's marker was still there (so it was not a full page load).
    await page.addInitScript(() => {
      document.addEventListener('astro:after-swap', () => {
        if (!location.pathname.startsWith('/dashboard')) return
        const marker = (window as Window & { __e2eDocument?: string }).__e2eDocument
        sessionStorage.setItem('__swappedInto', `${location.pathname} marker=${marker}`)
      })
    })

    await page.goto('/login')
    await expect(page.getByLabel('Email')).toBeVisible()
    await markDocument(page)

    await page.evaluate(() => {
      const link = document.createElement('a')
      link.href = '/dashboard'
      link.textContent = 'Go to dashboard'
      document.body.append(link)
    })
    await page.getByRole('link', { name: 'Go to dashboard' }).click()
    return { meCount: () => meCount }
  }

  async function expectBackOnLogin(page: Page, meCount: () => number): Promise<void> {
    // Polled across the redirect: a read that hits a document mid-navigation
    // is retried rather than failing the test.
    await expect
      .poll(
        () => recorded(page, '__swappedInto').catch(() => 'navigating'),
        'the move was a client-side swap'
      )
      .toBe('/dashboard marker=kept')
    await expect(page).toHaveURL(/\/login\/?$/)
    await expect(page.getByLabel('Email')).toBeVisible()
    await expect(page.getByRole('navigation', { name: /dashboard/i })).toHaveCount(0)
    await expect(page.getByRole('heading', { name: 'Home' })).toHaveCount(0)
    expect(meCount(), 'no account check was made').toBe(0)
  }

  test('from a public page with no session hint, still ends on /login', async ({ page }) => {
    const { meCount } = await followLinkToDashboard(page)
    await expectBackOnLogin(page, meCount)
  })

  test('if the head script does not run on the swap, the header backstop still ends on /login', async ({
    page,
  }) => {
    let stubbed = 0
    await page.route('**/scripts/dashboard-session-hint.js', async (route) => {
      stubbed += 1
      await fulfill(route, { contentType: 'text/javascript', body: '' })
    })
    const { meCount } = await followLinkToDashboard(page)
    await expectBackOnLogin(page, meCount)
    expect(stubbed, 'the swap fetched the (stubbed) head script').toBeGreaterThan(0)
  })
})
