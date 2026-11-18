// auth-abuse: a credential-stuffing burst and code guessing across many
// accounts, for checking the auth alerts end to end (OT-007 design D6).
//
// - stuffing: 40 wrong-password sign-ins against seeded Members over 2 minutes
//   (AuthSignInFailuresHigh: more than 30 in 15 minutes).
// - guessing: 6 fresh signups, each email code guessed wrongly until it locks,
//   then once more (AuthCodeLockoutsHigh: more than 5 lockouts in an hour).
//
// After a run, both alerts fire within about a minute (vmalert UI, or
// Alertmanager on http://localhost:9093) and stay up for 15 minutes and an hour.
// Locally Alertmanager has no email receiver, so nothing is sent.
// Only errors fail the run (the expected 401s and 400s aren't errors). Latency
// is reported but not judged: this checks alerting, not speed.
import exec from 'k6/execution'
import { Client } from '../lib/client.js'
import { thresholdsFor } from '../lib/budgets.js'
import { summaryFor } from '../lib/summary.js'
import { guessCodeUntilLocked, requireSeeded, wrongSignIn } from '../flows/auth.js'

const STUFFING = 40
const ACCOUNTS = 6

// One address prefix per run, so every run locks fresh codes.
const RUN = __ENV.LOAD_RUN_ID || `${Date.now().toString(36)}`

export const options = {
  scenarios: {
    stuffing: {
      executor: 'constant-arrival-rate',
      exec: 'stuffing',
      rate: STUFFING,
      timeUnit: '2m',
      duration: '2m',
      preAllocatedVUs: 4,
      tags: { phase: 'measure' },
    },
    guessing: {
      executor: 'per-vu-iterations',
      exec: 'guessing',
      vus: ACCOUNTS,
      iterations: 1,
      maxDuration: '2m',
      tags: { phase: 'measure' },
    },
  },
  thresholds: withoutLatencyLimits(
    thresholdsFor('auth-abuse', [
      'POST /api/v1/auth/login',
      'POST /api/v1/auth/register',
      'POST /api/v1/auth/verify-email',
    ])
  ),
  summaryTrendStats: ['avg', 'min', 'med', 'p(95)', 'p(99)', 'max', 'count'],
  setupTimeout: '30s',
}

/** Keep each route's latency in the summary, with a limit that always passes. */
function withoutLatencyLimits(thresholds) {
  for (const key of Object.keys(thresholds)) {
    if (key.startsWith('http_req_duration{')) thresholds[key] = ['max>=0']
  }
  return thresholds
}

export const handleSummary = summaryFor('auth-abuse', 120000)

export function setup() {
  const problem = requireSeeded(new Client())
  if (problem) exec.test.abort(problem)
}

export function stuffing() {
  wrongSignIn(new Client())
}

export function guessing() {
  guessCodeUntilLocked(new Client(), `test-load-abuse-${RUN}-${__VU}@example.test`)
}
