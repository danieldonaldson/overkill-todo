// A k6 HTTP client that behaves like the web client (OT-004 design Decision 6):
// cookies via the VU's jar, the CSRF token from the last sign-in or refresh
// sent back on state-changing requests, and one device fingerprint per VU.
// Every request is tagged `name: "<METHOD> <route template>"`, matching the
// backend's route names, so k6 and backend numbers line up.
import http from 'k6/http'
import { check } from 'k6'
import { API } from './config.js'

// Module scope runs once per VU, so each VU is its own "device".
const fingerprint = `load-${__VU}-${Math.random().toString(36).slice(2, 12)}`

const STATE_CHANGING = new Set(['POST', 'PUT', 'PATCH', 'DELETE'])

export class Client {
  constructor() {
    this.csrfToken = null
  }

  /**
   * Send a request to `${API}${route}`.
   * @param {string} method
   * @param {string} route route template, e.g. `/auth/me`; also the request's name
   * @param {object} [opts]
   * @param {object} [opts.body] JSON body
   * @param {number[]} [opts.expect] statuses that are not failures (default [200])
   * @param {boolean} [opts.fingerprint] send X-Device-Fingerprint
   * @param {string} [opts.path] actual path when it differs from the template
   */
  request(method, route, opts = {}) {
    const expect = opts.expect || [200]
    const headers = { Accept: 'application/json' }
    if (opts.body !== undefined) headers['Content-Type'] = 'application/json'
    if (opts.fingerprint) headers['X-Device-Fingerprint'] = fingerprint
    if (STATE_CHANGING.has(method) && this.csrfToken) headers['X-CSRF-Token'] = this.csrfToken

    const name = `${method} /api/v1${route}`
    const res = http.request(
      method,
      `${API}${opts.path || route}`,
      opts.body === undefined ? null : JSON.stringify(opts.body),
      {
        headers,
        tags: { name },
        responseCallback: http.expectedStatuses(...expect),
      }
    )
    check(res, { [`${name} status ${expect.join('|')}`]: (r) => expect.includes(r.status) })
    return res
  }

  /** Remember the CSRF token an auth response hands out. */
  takeSession(res) {
    if (res.status !== 200) return null
    const body = res.json()
    this.csrfToken = body.csrf_token || null
    return body
  }

  /** Forget the session, as the web client does after signing out. */
  reset() {
    this.csrfToken = null
    http.cookieJar().clear(API)
  }
}
