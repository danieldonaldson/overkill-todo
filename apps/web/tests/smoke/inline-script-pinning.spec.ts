import { createHash } from 'node:crypto'
import { expect, test } from '@playwright/test'
import { APP_BASE_URL, parseCsp } from './targets'

// The live counterpart to csp.spec.ts.
//
// csp.spec.ts hashes the inline scripts in the LOCAL build and checks them
// against the LOCAL nginx.conf. Both halves can be right while production
// serves something else entirely — a stale image, an un-promoted digest, or an
// edge that rewrites the HTML. This hashes the inline scripts in the response
// production actually returns, against the CSP header returned with that same
// response, so the two can never be out of step with each other.
//
// A failure here means hydration is broken in production right now: an inline
// script the browser is being told to refuse.

function inlineScripts(html: string): string[] {
  const out: string[] = []
  const re = /<script(\b[^>]*)>([\s\S]*?)<\/script>/g
  let m: RegExpExecArray | null
  while ((m = re.exec(html)) !== null) {
    if (!/\bsrc\s*=/.test(m[1])) out.push(m[2])
  }
  return out
}

function sha256(content: string): string {
  return `'sha256-${createHash('sha256').update(content, 'utf8').digest('base64')}'`
}

// Cloudflare JavaScript Detections injects this bootstrap at the edge. Its
// embedded token changes per request, so its hash can never be pinned — it is
// blocked by design and does not affect the app. Excluded by content, which is
// stable, rather than by hash, which is not.
function isCloudflareInjected(script: string): boolean {
  return script.includes('__CF$cv$params') || script.includes('cdn-cgi/challenge-platform')
}

// Every page the CSP has to cover. A route added to the dashboard without its
// bootstrap being re-hashed shows up here as a failure rather than as a blank
// page a user reports.
const PAGES = [
  '/',
  '/login/',
  '/register/',
  '/forgot-password/',
  '/reset-password/',
  '/verify-email/',
  '/set-password/',
  '/dashboard/',
]

for (const path of PAGES) {
  test(`every inline script served on ${path} is pinned by the CSP served with it`, async ({
    request,
  }) => {
    const res = await request.get(`${APP_BASE_URL}${path}`)
    expect(res.status(), `${path} is served`).toBe(200)

    const csp = parseCsp(res.headers()['content-security-policy'])
    const scriptSrc = csp.get('script-src') ?? []
    expect(scriptSrc.length, `${path} is served with a script-src`).toBeGreaterThan(0)

    const ours = inlineScripts(await res.text()).filter((s) => !isCloudflareInjected(s))
    const unpinned = ours.map(sha256).filter((hash) => !scriptSrc.includes(hash))

    expect(
      unpinned,
      `inline scripts on ${path} are blocked in production; add these hashes to nginx.conf and promote`,
    ).toEqual([])
  })
}
