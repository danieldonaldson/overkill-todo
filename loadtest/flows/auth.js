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
