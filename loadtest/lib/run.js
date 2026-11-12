// Builds a scenario's k6 options: an optional warm-up phase, then a measured
// phase, both open-model (arrival rate), so a slow server shows up as latency
// instead of fewer requests (OT-004 design Decision 4).
import { thresholdsFor } from './budgets.js'

const UNIT_MS = { ms: 1, s: 1000, m: 60000, h: 3600000 }

/** '90s' | '3m' | '1h30m' → milliseconds. */
export function durationMs(d) {
  let total = 0
  const re = /(\d+(?:\.\d+)?)(ms|s|m|h)/g
  let m
  while ((m = re.exec(d))) total += parseFloat(m[1]) * UNIT_MS[m[2]]
  if (total === 0) throw new Error(`Bad duration "${d}"`)
  return total
}

/**
 * A constant-rate scenario.
 * @param {object} p
 * @param {string} p.scenario scenario name (matches the file name)
 * @param {string[]} p.routes routes it exercises; each needs a budget
 * @param {number} p.rate iterations per second
 * @param {string} p.duration measured duration, e.g. '3m'
 * @param {string} [p.warmup] warm-up duration at the same rate, excluded from thresholds
 * @param {number} [p.vus] pre-allocated VUs
 * @param {number} [p.maxVUs] VU ceiling; when reached, iterations are dropped
 * @returns {{options: object, plannedMs: number}}
 */
export function constantRun(p) {
  const vus = p.vus ?? Math.max(2, Math.ceil(p.rate))
  const base = {
    executor: 'constant-arrival-rate',
    rate: p.rate,
    timeUnit: '1s',
    preAllocatedVUs: vus,
    maxVUs: p.maxVUs ?? vus * 4,
  }
  const scenarios = {}
  let start = 0
  if (p.warmup) {
    scenarios.warmup = { ...base, duration: p.warmup, tags: { phase: 'warmup' } }
    start = durationMs(p.warmup)
  }
  scenarios.measure = {
    ...base,
    duration: p.duration,
    startTime: `${start}ms`,
    tags: { phase: 'measure' },
  }
  return {
    options: {
      scenarios,
      thresholds: thresholdsFor(p.scenario, p.routes),
      summaryTrendStats: ['avg', 'min', 'med', 'p(95)', 'p(99)', 'max', 'count'],
      // Setup's seed check signs in once at production hashing cost.
      setupTimeout: '30s',
    },
    plannedMs: start + durationMs(p.duration),
  }
}
