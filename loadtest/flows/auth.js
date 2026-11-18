// Auth flows: what a Member does with a session. Todo flows go in a sibling
// file (flows/todos.js) once todo endpoints exist, and scenarios mix them in.
import { USERS, TEST_PASSWORD, testEmail } from '../lib/config.js'

/** Seeded Member for this VU, so each VU keeps its own session. */
export function memberForVu() {
  return testEmail((__VU - 1) % USERS)
}

export function signIn(client, email = memberForVu(), expect = [200]) {
  const res = client.request('POST', '/auth/login', {
    body: { email, password: TEST_PASSWORD },
    fingerprint: true,
    expect,
  })
  client.takeSession(res)
  return res
}

export function me(client) {
  return client.request('GET', '/auth/me')
}

export function refresh(client) {
  const res = client.request('POST', '/auth/refresh', { fingerprint: true })
  client.takeSession(res)
  return res
}

export function signOut(client) {
  const res = client.request('POST', '/auth/logout')
  client.reset()
  return res
}

export function ready(client) {
  return client.request('GET', '/health/ready')
}

/** One auth-mix iteration: a short Member session (design Decision 4). */
export function authMixIteration(client) {
  if (signIn(client).status !== 200) {
    client.reset()
    return
  }
  for (let i = 0; i < 5; i++) me(client)
  refresh(client)
  for (let i = 0; i < 2; i++) me(client)
  signOut(client)
}

/**
 * Scenario `setup()` guard: sign in as the last seeded Member, and abort the
 * run with a clear message if seeding hasn't been done.
 */
export function requireSeeded(client) {
  const last = testEmail(USERS - 1)
  const res = client.request('POST', '/auth/login', {
    body: { email: last, password: TEST_PASSWORD },
    fingerprint: true,
    expect: [200, 401],
  })
  client.reset()
  return res.status === 200
    ? null
    : `Test Member ${last} can't sign in (HTTP ${res.status}). Run \`just load-seed ${USERS}\` first.`
}

// ── Abuse (OT-007) ──────────────────────────────────────────────────────────────
// What an attacker does, for checking the auth alerts end to end. Not part of
// any normal-use scenario.

/** Sign in as a seeded Member with a wrong password (credential stuffing). */
export function wrongSignIn(client, email = memberForVu()) {
  const res = client.request('POST', '/auth/login', {
    body: { email, password: 'not-the-password-1' },
    fingerprint: true,
    expect: [401],
  })
  client.reset()
  return res
}

/**
 * Register a fresh signup and guess its email code wrongly until it locks
 * (`attempts` wrong guesses), then once more to be refused as locked. The
 * address starts `test-load-` so `just load-reset` removes it.
 */
export function guessCodeUntilLocked(client, email, attempts = 5) {
  client.request('POST', '/auth/register', { body: { email } })
  // The dev code for `test-*` addresses is 123456, so this is always wrong.
  for (let i = 0; i < attempts + 1; i++) {
    client.request('POST', '/auth/verify-email', {
      body: { email, code: '000000' },
      expect: [400],
    })
  }
  client.reset()
}
