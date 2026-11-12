// Run configuration from the environment, with the local-only guard
// (OT-004 design Decision 8). Evaluated in the init context, so a bad target
// stops the run before any request is sent.
import exec from 'k6/execution'

const LOCAL_HOSTS = new Set(['edge', 'localhost', '127.0.0.1'])

export const BASE_URL = (__ENV.LOAD_BASE_URL || 'http://edge:8080').replace(/\/+$/, '')
export const API = `${BASE_URL}/api/v1`

/** Number of seeded test Members (`just load-seed <n>`). */
export const USERS = parseInt(__ENV.LOAD_USERS || '500', 10)

/** Fixed password for every seeded test Member. Local test data only. */
export const TEST_PASSWORD = 'load-test-password-1'

/** Seeded Member `i`. The `test-` prefix gets the fixed dev verification code. */
export function testEmail(i) {
  return `test-load-${i}@example.test`
}

function hostOf(url) {
  const m = /^[a-z]+:\/\/(\[[^\]]+\]|[^/:]+)/i.exec(url)
  return m ? m[1].toLowerCase() : ''
}

export function assertLocalTarget() {
  const host = hostOf(BASE_URL)
  if (!LOCAL_HOSTS.has(host) && __ENV.LOAD_ALLOW_REMOTE !== '1') {
    exec.test.abort(
      `Refusing to send load to "${host || BASE_URL}": only edge, localhost and 127.0.0.1 ` +
        'are allowed. Set LOAD_ALLOW_REMOTE=1 to override for this run.'
    )
  }
}

if (!Number.isInteger(USERS) || USERS < 1) {
  exec.test.abort(`LOAD_USERS must be a positive integer, got "${__ENV.LOAD_USERS}"`)
}
assertLocalTarget()
