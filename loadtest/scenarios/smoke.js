// smoke: one simulated Member, every load-tested route, zero errors allowed.
// Checks the scripts and the stack work before a heavier run.
import exec from 'k6/execution'
import { Client } from '../lib/client.js'
import { constantRun } from '../lib/run.js'
import { summaryFor } from '../lib/summary.js'
import { me, ready, refresh, requireSeeded, signIn, signOut } from '../flows/auth.js'

const run = constantRun({
  scenario: 'smoke',
  routes: [
    'POST /api/v1/auth/login',
    'GET /api/v1/auth/me',
    'POST /api/v1/auth/refresh',
    'POST /api/v1/auth/logout',
    'GET /api/v1/health/ready',
  ],
  rate: 1,
  duration: '30s',
  vus: 2,
})

export const options = run.options
export const handleSummary = summaryFor('smoke', run.plannedMs)

export function setup() {
  const problem = requireSeeded(new Client())
  if (problem) exec.test.abort(problem)
}

export default function () {
  const client = new Client()
  ready(client)
  if (signIn(client).status !== 200) return client.reset()
  me(client)
  // Two refreshes in a row: each must use the token the previous one issued.
  refresh(client)
  refresh(client)
  me(client)
  signOut(client)
}
