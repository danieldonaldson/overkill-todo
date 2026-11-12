#!/usr/bin/env node
// Post-run report (OT-004 design Decision 10): compares a result with its
// scenario's baseline. k6 thresholds decide pass/fail; this adds what they
// can't express. Run by loadtest/run.sh after k6 exits.
//
//   node loadtest/report.mjs <result.json>
//   node loadtest/report.mjs --set-baseline <scenario>
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync } from 'node:fs'
import { basename, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const resultsDir = join(here, 'results')
const baselinesDir = join(here, 'baselines')
const baselinePath = (scenario) => join(baselinesDir, `${scenario}.json`)
const read = (p) => JSON.parse(readFileSync(p, 'utf8'))

function compare(result) {
  const path = baselinePath(result.scenario)
  if (!existsSync(path)) {
    console.log(`No baseline for ${result.scenario} yet; judged on its limits alone.`)
    console.log(`Set one with \`just load-baseline ${result.scenario}\` after a good run.`)
    return
  }
  const base = read(path)
  console.log(`p95 against the baseline from ${base.started} (commit ${base.meta?.commit ?? '?'}):`)
  const rows = [['route', 'baseline ms', 'now ms', 'change']]
  for (const [route, r] of Object.entries(result.routes).sort()) {
    const b = base.routes?.[route]
    if (!r.count) continue
    if (!b?.p95) {
      rows.push([route, '-', r.p95.toFixed(1), 'new'])
      continue
    }
    const pct = ((r.p95 - b.p95) / b.p95) * 100
    rows.push([
      route,
      b.p95.toFixed(1),
      r.p95.toFixed(1),
      `${pct >= 0 ? '+' : ''}${pct.toFixed(1)}%`,
    ])
  }
  const w = rows[0].map((_, i) => Math.max(...rows.map((row) => row[i].length)))
  for (const row of rows)
    console.log(row.map((c, i) => (i === 0 ? c.padEnd(w[i]) : c.padStart(w[i]))).join('  '))
  const differs = [
    'backend_cpuset',
    'backend_memory_mb',
    'db_cpuset',
    'argon2',
    'k6_cpuset',
  ].filter((k) => (base.meta?.[k] ?? '') !== (result.meta?.[k] ?? ''))
  if (differs.length) {
    console.log(
      `Warning: stack settings differ from the baseline (${differs.join(', ')}); not like for like.`
    )
  }
}

function setBaseline(scenario) {
  const candidates = existsSync(resultsDir)
    ? readdirSync(resultsDir)
        .filter((f) => f.startsWith(`${scenario}-`) && f.endsWith('.json'))
        .sort()
        .reverse()
    : []
  for (const f of candidates) {
    const r = read(join(resultsDir, f))
    if (r.scenario !== scenario) continue
    if (!r.complete) {
      console.error(
        `The latest ${scenario} run (${f}) was interrupted; it can't be a baseline. Run it again.`
      )
      process.exit(1)
    }
    mkdirSync(baselinesDir, { recursive: true })
    copyFileSync(join(resultsDir, f), baselinePath(scenario))
    console.log(
      `Baseline for ${scenario} set from ${f} (${r.passed ? 'passed' : 'FAILED its limits'}).`
    )
    return
  }
  console.error(`No ${scenario} results in ${resultsDir}. Run \`just load ${scenario}\` first.`)
  process.exit(1)
}

const [arg, value] = process.argv.slice(2)
if (arg === '--set-baseline' && value) {
  setBaseline(value)
} else if (arg && !arg.startsWith('--')) {
  const result = read(arg)
  if (!result.complete)
    console.log(`(${basename(arg)} is incomplete; compared for information only.)`)
  compare(result)
} else {
  console.error('usage: report.mjs <result.json> | --set-baseline <scenario>')
  process.exit(2)
}
