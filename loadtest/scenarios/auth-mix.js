// auth-mix: short Member sessions (sign in, check the session, refresh, sign
// out) at a fixed rate. Sign-in carries production hashing cost.
import exec from 'k6/execution'
import { Client } from '../lib/client.js'
import { constantRun } from '../lib/run.js'
import { summaryFor } from '../lib/summary.js'
import { authMixIteration, requireSeeded } from '../flows/auth.js'

const run = constantRun({
  scenario: 'auth-mix',
  routes: [
    'POST /api/v1/auth/login',
    'GET /api/v1/auth/me',
    'POST /api/v1/auth/refresh',
    'POST /api/v1/auth/logout',
  ],
  rate: 10,
  warmup: '30s',
  duration: '3m',
  vus: 20,
  maxVUs: 100,
})

export const options = run.options
export const handleSummary = summaryFor('auth-mix', run.plannedMs)

export function setup() {
  const problem = requireSeeded(new Client())
  if (problem) exec.test.abort(problem)
}

export default function () {
  authMixIteration(new Client())
}
