import { expect, test } from '@playwright/test'
import { APP_BASE_URL } from './targets'

// Part 3 — "is it up, and is it up at the right address". Every assertion here
// is a plain GET; nothing in this file changes state.

test.describe('reachability', () => {
  test('the API reports healthy', async ({ request }) => {
    const res = await request.get(`${APP_BASE_URL}/api/v1/health`)
    expect(res.status()).toBe(200)
    expect(await res.json()).toEqual({ service: 'app', status: 'ok' })
  })

  test('the API reports ready, with the database reachable', async ({ request }) => {
    const res = await request.get(`${APP_BASE_URL}/api/v1/health/ready`)
    expect(res.status()).toBe(200)
    // db:ok is the half that matters — the pod can be up while CNPG is not.
    expect(await res.json()).toEqual({ db: 'ok', status: 'ready' })
  })

  test('the dashboard is served over HTTPS', async ({ request }) => {
    const res = await request.get(`${APP_BASE_URL}/`)
    expect(res.status()).toBe(200)
    expect(res.url()).toMatch(/^https:/)
  })

  test('plain HTTP is redirected to HTTPS, without following it', async ({ request }) => {
    const insecure = APP_BASE_URL.replace(/^https:/, 'http:')
    const res = await request.get(`${insecure}/`, { maxRedirects: 0 })
    // Edges usually answer 308; 301 is equally acceptable. What must not happen
    // is a 200 — that would mean plaintext is being served.
    expect([301, 302, 307, 308]).toContain(res.status())
    expect(res.headers()['location']).toMatch(/^https:/)
  })

})
