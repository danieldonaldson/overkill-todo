import { devices, expect, test, type Page } from '@playwright/test'
import { hide, ready, setup } from './helpers/telemetry'

// Web Vitals (OT-006 slice 3, specs/observability "Real-user performance is
// measured"). The API is mocked; /api/v1/client-telemetry records each report.

/**
 * Make the next clicks slow (a handler blocking the main thread), so the
 * interaction is long enough to be measured (over 40 ms) and rated poor.
 */
async function slowClicks(page: Page, ms = 600) {
  await page.evaluate((ms) => {
    document.addEventListener('click', () => {
      const until = performance.now() + ms
      while (performance.now() < until) {
        // busy
      }
    })
  }, ms)
}

test.describe('Web Vitals', () => {
  test('Home then Settings: load vitals for Home, INP and CLS for each visit', async ({ page }) => {
    const { vitals, reports, cspViolations } = await setup(page, { signedIn: true })
    await page.goto('/dashboard')
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await ready(page)

    await page.getByRole('link', { name: 'Settings' }).click()
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await ready(page)
    // An interaction on Settings, slow enough to be measured.
    await slowClicks(page)
    await page.getByRole('heading', { name: 'Settings' }).click()
    await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => setTimeout(r, 100))))
    await hide(page)

    await expect
      .poll(() =>
        vitals()
          .filter(([, p]) => p === '/dashboard/settings')
          .map(([n]) => n)
          .sort()
      )
      .toEqual(['CLS', 'INP'])
    // Home's INP is the click on the Settings link that ended its visit.
    await expect
      .poll(() =>
        vitals()
          .filter(([, p]) => p === '/dashboard')
          .map(([n]) => n)
          .sort()
      )
      .toEqual(['CLS', 'FCP', 'INP', 'LCP', 'TTFB'])
    // Load metrics belong to the full load only.
    expect(
      vitals().filter(([n, p]) => ['LCP', 'FCP', 'TTFB'].includes(n) && p !== '/dashboard')
    ).toEqual([])

    const inp = reports
      .filter((r) => r.page === '/dashboard/settings')
      .flatMap((r) => r.events)
      .find((e) => e.name === 'INP')!
    expect(inp.rating).toBe('poor')
    expect(inp.value).toBeGreaterThan(500)
    // A poor INP names the element by tag, ID and classes, never its text.
    expect(inp).toHaveProperty('attribution')
    expect(JSON.stringify(inp)).not.toContain('Settings')
    expect(cspViolations).toEqual([])
  })

  test('a sample rate of 0 sends no vitals but still sends page loads and errors', async ({
    page,
  }) => {
    const { vitals, pageLoads, events } = await setup(page, { sampleRate: '0' })
    await page.goto('/login')
    await ready(page)
    await page.evaluate(() => {
      setTimeout(() => {
        throw new Error('still reported')
      })
    })
    await page.getByRole('textbox', { name: 'Email' }).click()
    await hide(page)

    await expect.poll(() => pageLoads().map(([p]) => p)).toEqual(['/login'])
    await expect.poll(() => events().map((e) => e.message)).toEqual(['Error: still reported'])
    expect(vitals()).toEqual([])
  })

  test('a junk sample rate measures every page load', async ({ page }) => {
    const { vitals } = await setup(page, { sampleRate: 'ten percent' })
    await page.goto('/login')
    await ready(page)
    await hide(page)
    await expect.poll(() => vitals().map(([n]) => n)).toContain('CLS')
  })
})

test.describe('Web Vitals on a phone', () => {
  const { viewport, userAgent, deviceScaleFactor, isMobile, hasTouch } = devices['Pixel 7']
  test.use({ viewport, userAgent, deviceScaleFactor, isMobile, hasTouch })

  test('switching to another app sends the vitals collected so far', async ({ page }) => {
    const { vitals, reports } = await setup(page)
    await page.goto('/register')
    await ready(page)
    await page.getByRole('textbox', { name: 'Email' }).tap()
    expect(vitals()).toEqual([])
    await hide(page)

    await expect
      .poll(() =>
        vitals()
          .map(([n]) => n)
          .sort()
      )
      .toEqual(expect.arrayContaining(['CLS', 'FCP', 'LCP', 'TTFB']))
    expect(new Set(reports.map((r) => r.page))).toEqual(new Set(['/register']))
  })
})
