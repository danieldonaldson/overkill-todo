// Seeds LOAD_USERS verified test Members through the real sign-up flow
// (OT-004 design Decision 7). Safe to repeat: a Member who can already sign in
// is skipped, and register reuses an incomplete signup, so a partial seed
// resumes where it stopped.
import exec from 'k6/execution'
import { Counter } from 'k6/metrics'
import { USERS, TEST_PASSWORD, testEmail } from './lib/config.js'
import { Client } from './lib/client.js'

// In DEV_MODE, `test-*` addresses always get this verification code.
const DEV_TEST_CODE = '123456'

const created = new Counter('seed_created')
const reused = new Counter('seed_reused')
const failed = new Counter('seed_failed')

export const options = {
  scenarios: {
    seed: {
      executor: 'shared-iterations',
      vus: Math.min(8, USERS),
      iterations: USERS,
      maxDuration: '30m',
    },
  },
  // Any failed account fails the command.
  thresholds: { seed_failed: ['count==0'] },
  summaryTrendStats: ['avg', 'p(95)', 'max'],
}

function fail(email, step, res) {
  failed.add(1)
  console.error(`${email}: ${step} failed with HTTP ${res.status}: ${res.body}`)
}

export default function () {
  const i = exec.scenario.iterationInTest
  const email = testEmail(i)
  const client = new Client()

  const login = client.request('POST', '/auth/login', {
    body: { email, password: TEST_PASSWORD },
    fingerprint: true,
    expect: [200, 401],
  })
  if (login.status === 200) {
    reused.add(1)
    client.reset()
    return
  }

  const reg = client.request('POST', '/auth/register', { body: { email } })
  if (reg.status !== 200) return fail(email, 'register', reg)

  const verify = client.request('POST', '/auth/verify-email', {
    body: { email, code: DEV_TEST_CODE },
  })
  if (verify.status !== 200) return fail(email, 'verify-email', verify)
  const signupToken = verify.json('signup_token')
  if (!signupToken) {
    // Has a password already but the sign-in above failed: not ours to fix.
    return fail(email, 'verify-email (no signup token; account has another password?)', verify)
  }

  const done = client.request('POST', '/auth/complete-signup', {
    body: { signup_token: signupToken, email, password: TEST_PASSWORD },
    fingerprint: true,
  })
  if (done.status !== 200) return fail(email, 'complete-signup', done)
  created.add(1)
  client.reset()
}

export function handleSummary(data) {
  const n = (k) => data.metrics[k]?.values.count ?? 0
  return {
    stdout:
      `\nSeeded ${USERS} test Members: ${n('seed_created')} created, ` +
      `${n('seed_reused')} reused, ${n('seed_failed')} failed.\n`,
  }
}
