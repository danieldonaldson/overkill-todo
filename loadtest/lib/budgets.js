// Latency and error budgets: the single source for every scenario's
// thresholds (OT-004 design Decision 5). Values are milliseconds.
//
// Rule: p95 = max(1.5 × worst p95, 25), p99 = max(1.5 × worst p99, 50), rounded
// up, over the slice 1 auth-mix variance runs (2026-11-12, physical-core
// layout, worst of 5 back-to-back runs). The floors keep sub-millisecond routes
// from failing on noise. Sign-up steps have no measurements yet (register-burst
// comes in slice 2), so they keep provisional values.

export const ROUTE_BUDGETS = {
  'GET /api/v1/auth/me': { p95: 25, p99: 50 }, // worst 1.52 / 2.59
  'GET /api/v1/health/ready': { p95: 25, p99: 50 }, // ~1 ms in smoke
  'POST /api/v1/auth/login': { p95: 167, p99: 233 }, // worst 111.2 / 155.0
  'POST /api/v1/auth/refresh': { p95: 25, p99: 50 }, // worst 11.3 / 21.2
  'POST /api/v1/auth/logout': { p95: 25, p99: 50 }, // worst 8.3 / 19.3
  'POST /api/v1/auth/register': { p95: 400, p99: 800 }, // provisional
  'POST /api/v1/auth/verify-email': { p95: 400, p99: 800 }, // provisional
  'POST /api/v1/auth/complete-signup': { p95: 400, p99: 800 }, // provisional
}

/** Highest allowed share of failed requests, per scenario. */
export const ERROR_BUDGETS = {
  smoke: 0,
  'read-baseline': 0.001,
  others: 0.01,
}

/**
 * k6 `thresholds` for a scenario: p95/p99 per route and the error rate, over
 * the measured phase only, and no dropped iterations (the generator kept up).
 * A route without a budget is a mistake, so it throws.
 */
export function thresholdsFor(scenario, routes) {
  const errorBudget = ERROR_BUDGETS[scenario] ?? ERROR_BUDGETS.others
  const t = {
    [`http_req_failed{phase:measure}`]: [errorBudget === 0 ? 'rate==0' : `rate<${errorBudget}`],
    dropped_iterations: ['count==0'],
  }
  for (const route of routes) {
    const b = ROUTE_BUDGETS[route]
    if (!b) throw new Error(`No budget for route "${route}" in lib/budgets.js`)
    t[`http_req_duration{name:${route},phase:measure}`] = [`p(95)<${b.p95}`, `p(99)<${b.p99}`]
    // Always passes: it only makes k6 track the per-route error rate for the summary.
    t[`http_req_failed{name:${route},phase:measure}`] = ['rate>=0']
  }
  return t
}
