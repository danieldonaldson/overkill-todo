import { expect, test, type Page } from '@playwright/test'
import { APP_BASE_URL } from './targets'

// Headers being present does not prove the app still works under them — a CSP
// can be correctly delivered and still break hydration or a third-party widget.
// These tests load the real production pages in a real browser and assert on
// what the browser did with them.

type Violation = { directive: string; blockedURI: string; sample: string }

// Collect CSP violations as the browser reports them. The DOM event carries a
// script sample, which is what makes the allow-list below possible: without it,
// Cloudflare's blocked inline script and one of our own unpinned inline scripts
// are indistinguishable (both report blockedURI "inline").
async function collectViolations(page: Page): Promise<Violation[]> {
  const violations: Violation[] = []
  await page.exposeFunction('__reportCspViolation', (v: Violation) => {
    violations.push(v)
  })
  await page.addInitScript(() => {
    document.addEventListener('securitypolicyviolation', (e) => {
      const evt = e as SecurityPolicyViolationEvent
      ;(
        window as unknown as { __reportCspViolation: (v: Violation) => void }
      ).__reportCspViolation({
        directive: evt.effectiveDirective || evt.violatedDirective,
        blockedURI: evt.blockedURI,
        sample: evt.sample ?? '',
      })
    })
  })
  return violations
}

// Known and accepted when the site sits behind Cloudflare: Cloudflare JavaScript
// Detections injects an inline bootstrap carrying a PER-REQUEST token, so its
// hash differs on every response and can never be pinned. It is passive bot
// scoring — the app is unaffected — and the accepted remedy is to switch JSD
// off for this host, not to weaken the CSP.
//
// Inline violations are NOT judged here. Chromium only fills the event's
// `sample` field when the CSP carries `report-sample`, which ours does not, so
// every blocked inline script arrives as an indistinguishable
// {blockedURI: "inline", sample: ""} — Cloudflare's and one of ours alike.
// Judging them here would either accept a real regression or fail forever.
// inline-script-pinning.spec.ts covers inline scripts exactly instead, by
// hashing them out of the live HTML. What is left for this test is external
// origins, where blockedURI names the resource and the verdict is unambiguous.
function isAccepted(v: Violation): boolean {
  return v.blockedURI === 'inline' || v.blockedURI.includes('cdn-cgi/challenge-platform')
}

// Deliberately NOT `waitUntil: 'networkidle'`: Turnstile's challenge-platform
// holds connections open, so the network never goes idle and every navigation
// times out. Wait on what the test actually cares about instead.
async function waitForTurnstile(page: Page): Promise<void> {
  await expect
    .poll(() => page.frames().filter((f) => f.url().includes('challenges.cloudflare.com')).length, {
      message: 'the Turnstile widget iframe is present in the frame tree',
      timeout: 20_000,
    })
    .toBeGreaterThan(0)
}

test.describe('the production dashboard runs under its own CSP', () => {
  test('the login page loads with no external resource blocked by the CSP', async ({ page }) => {
    const violations = await collectViolations(page)

    await page.goto(`${APP_BASE_URL}/login/`)
    // Islands hydrate after first paint, and a CSP violation is reported when
    // the blocked resource is reached — so give the page a real signal to
    // settle on rather than a bare load event. Turnstile appearing means the
    // scripts CSP allows have run.
    await expect(page.getByLabel(/email/i)).toBeVisible()
    await waitForTurnstile(page)

    const unexpected = violations.filter((v) => !isAccepted(v))
    expect(
      unexpected,
      'the CSP is blocking an external resource the app needs — check the third-party origins in script-src/connect-src/frame-src',
    ).toEqual([])
  })

  test('Turnstile renders on the login page', async ({ page }) => {
    await page.goto(`${APP_BASE_URL}/login/`)

    // Turnstile mounts into a SHADOW ROOT, so document.querySelectorAll('iframe')
    // finds nothing and a naive check wrongly concludes the widget is broken.
    // Look at the browser's frame tree instead.
    await waitForTurnstile(page)
  })

  test('the login form is hydrated and interactive', async ({ page }) => {
    await page.goto(`${APP_BASE_URL}/login/`)

    // Islands hydrate after first paint. Filling a field proves the bundle
    // executed — i.e. that the hashed inline bootstrap was allowed to run.
    const email = page.getByLabel(/email/i)
    await expect(email).toBeVisible()
    await email.fill('smoke-check@example.com')
    await expect(email).toHaveValue('smoke-check@example.com')
    // Nothing is submitted: this suite never creates state in production.
  })

  test('the dashboard redirects an unauthenticated visitor to login', async ({ page }) => {
    await page.goto(`${APP_BASE_URL}/dashboard/`)
    await expect(page).toHaveURL(/\/login/, { timeout: 20_000 })
  })
})
