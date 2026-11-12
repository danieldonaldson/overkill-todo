// End-of-run summary (OT-004 design Decision 10): a per-route table on stdout,
// the limits that were exceeded, and a JSON result in /results for
// report.mjs and `just load-baseline`.

const RESULTS_DIR = __ENV.LOAD_RESULTS_DIR || '/results'

/** LOAD_META_FOO_BAR=x → { foo_bar: 'x' }, set by the `just load` recipe. */
function meta() {
  const out = {}
  for (const [k, v] of Object.entries(__ENV)) {
    if (k.startsWith('LOAD_META_')) out[k.slice('LOAD_META_'.length).toLowerCase()] = v
  }
  return out
}

const SUB = /^(http_req_duration|http_req_failed)\{name:(.+),phase:measure\}$/

function perRoute(metrics) {
  const routes = {}
  for (const [key, m] of Object.entries(metrics)) {
    const hit = SUB.exec(key)
    if (!hit) continue
    const r = (routes[hit[2]] ??= {})
    if (hit[1] === 'http_req_duration') {
      Object.assign(r, {
        count: m.values.count,
        p50: m.values.med,
        p95: m.values['p(95)'],
        p99: m.values['p(99)'],
        max: m.values.max,
      })
    } else {
      r.error_rate = m.values.rate
    }
  }
  return routes
}

function exceeded(metrics) {
  const out = []
  for (const [key, m] of Object.entries(metrics)) {
    for (const [expr, t] of Object.entries(m.thresholds || {})) {
      if (t.ok) continue
      const stat = expr.split(/[<>=]/)[0]
      const measured =
        stat === 'count' ? m.values.count : stat === 'rate' ? m.values.rate : m.values[stat]
      out.push({ metric: key, limit: expr, measured })
    }
  }
  return out
}

const fmtMs = (v) => (v === undefined ? '-' : v < 10 ? v.toFixed(2) : v.toFixed(0))
const fmtPct = (v) => (v === undefined ? '-' : `${(v * 100).toFixed(2)}%`)

function table(routes) {
  const rows = [['route', 'count', 'errors', 'p50 ms', 'p95 ms', 'p99 ms', 'max ms']]
  for (const [name, r] of Object.entries(routes).sort()) {
    if (!r.count) continue
    rows.push([
      name,
      String(r.count),
      fmtPct(r.error_rate),
      fmtMs(r.p50),
      fmtMs(r.p95),
      fmtMs(r.p99),
      fmtMs(r.max),
    ])
  }
  const w = rows[0].map((_, i) => Math.max(...rows.map((row) => row[i].length)))
  return rows
    .map((row) => row.map((c, i) => (i === 0 ? c.padEnd(w[i]) : c.padStart(w[i]))).join('  '))
    .join('\n')
}

/** `export const handleSummary = summaryFor('smoke', plannedMs)` in a scenario. */
export function summaryFor(scenario, plannedMs) {
  return (data) => {
    const elapsedMs = data.state.testRunDurationMs
    // Small slack: k6 timing isn't exact, and an interrupted run is short by seconds at least.
    const complete = elapsedMs >= plannedMs - 1000
    const routes = perRoute(data.metrics)
    // Aborted before any measured request (e.g. the seed check failed): k6's
    // abort message says why; there are no results to keep.
    if (!Object.values(routes).some((r) => r.count)) {
      return { stdout: `\n── ${scenario}: no requests measured, nothing saved ──\n` }
    }
    const failed = exceeded(data.metrics)
    const dropped = data.metrics.dropped_iterations?.values.count ?? 0
    const total = data.metrics['http_req_failed{phase:measure}']?.values
    const started = new Date(Date.now() - elapsedMs).toISOString()

    const result = {
      scenario,
      started,
      complete,
      planned_ms: plannedMs,
      elapsed_ms: elapsedMs,
      passed: failed.length === 0,
      meta: meta(),
      error_rate: total?.rate ?? null,
      dropped_iterations: dropped,
      routes,
      exceeded: failed,
    }

    const lines = [
      '',
      `── ${scenario} ${complete ? '' : '(INCOMPLETE: interrupted before the planned duration) '}──`,
      table(routes),
      '',
      `error rate ${fmtPct(result.error_rate ?? undefined)}, dropped iterations ${dropped}`,
    ]
    if (dropped > 0) {
      lines.push(`The generator couldn't keep up: ${dropped} planned iterations were not started.`)
    }
    if (failed.length) {
      lines.push('Limits exceeded:')
      for (const f of failed) {
        const v = f.limit.startsWith('rate')
          ? fmtPct(f.measured)
          : f.limit.startsWith('count')
            ? f.measured
            : `${fmtMs(f.measured)} ms`
        lines.push(`  ${f.metric}: ${f.limit} (measured ${v})`)
      }
    } else {
      lines.push('All limits met.')
    }
    const m = result.meta
    if (m.backend_cpuset) {
      lines.push(
        `Stack: backend cpus ${m.backend_cpuset} (${m.backend_cpus}), ${m.backend_memory_mb} MB; ` +
          `db cpus ${m.db_cpuset}, ${m.db_memory_mb} MB; argon2 ${m.argon2}; commit ${m.commit}`
      )
    }
    if (m.dev_mode === 'true') {
      lines.push(
        'DEV_MODE=true: console email and no Turnstile/MX checks, so sign-up is a little cheaper than production.'
      )
    }
    lines.push('')

    const file =
      __ENV.LOAD_RESULT_FILE || `${RESULTS_DIR}/${scenario}-${started.replace(/[:.]/g, '-')}.json`
    return { stdout: lines.join('\n'), [file]: JSON.stringify(result, null, 2) }
  }
}
