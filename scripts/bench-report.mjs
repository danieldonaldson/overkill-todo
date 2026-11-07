#!/usr/bin/env node
// Compare the base and PR sides written by scripts/bench-compare.sh and print a
// markdown report (also appended to $GITHUB_STEP_SUMMARY in Actions).
//
// A change counts only when it is both larger than BENCH_THRESHOLD percent and
// outside the noise: the two sides' 95% confidence intervals must not overlap.
// With several rounds per side, each benchmark keeps its fastest round.
// Usage: bench-report.mjs <out-dir> <base-label>
import { appendFileSync, existsSync, readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'

const [outDir, baseLabel = 'base'] = process.argv.slice(2)
const threshold = Number(process.env.BENCH_THRESHOLD ?? 10) / 100
const inActions = process.env.GITHUB_ACTIONS === 'true'

/** @typedef {{ point: number, lo: number, hi: number }} Estimate  (nanoseconds) */

/** Keep the fastest estimate per benchmark across rounds. */
function keepFastest(results, name, est) {
  const prev = results.get(name)
  if (!prev || est.point < prev.point) results.set(name, est)
}

/** Criterion: <home>/<bench dirs...>/<prefix><round>/{benchmark,estimates}.json */
function readCriterion(home, prefix) {
  /** @type {Map<string, Estimate>} */
  const results = new Map()
  if (!existsSync(home)) return results
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue
      const path = join(dir, entry.name)
      if (entry.name.startsWith(prefix) && existsSync(join(path, 'estimates.json'))) {
        const { full_id: id } = JSON.parse(readFileSync(join(path, 'benchmark.json'), 'utf8'))
        // Median: robust to the occasional descheduled sample on shared runners.
        const { median } = JSON.parse(readFileSync(join(path, 'estimates.json'), 'utf8'))
        keepFastest(results, `backend: ${id}`, {
          point: median.point_estimate,
          lo: median.confidence_interval.lower_bound,
          hi: median.confidence_interval.upper_bound,
        })
      } else {
        walk(path)
      }
    }
  }
  walk(home)
  return results
}

/**
 * Vitest bench results <out>/<prefix><round>.json: mean (ms) ± rme (%), keyed
 * `web: <group> > <bench>`. Reads both formats, since the base side may predate
 * the Vitest 5 move: v5 JSON reporter (`testResults[].assertionResults[]
 * .benchmarks[].tasks[]`, group = test title) and v4 `--outputJson` (`files[]
 * .groups[].benchmarks[]`, group = describe name).
 */
function readVitest(dir, prefix) {
  /** @type {Map<string, Estimate>} */
  const results = new Map()
  /** @param {string} group @param {string} name @param {number} meanMs @param {number} rme */
  const add = (group, name, meanMs, rme) => {
    // Mean, not median: the browser timer's 0.1 ms resolution zeroes the
    // median of anything faster than that.
    const point = meanMs * 1e6
    const moe = (point * rme) / 100
    keepFastest(results, `web: ${group} > ${name}`, { point, lo: point - moe, hi: point + moe })
  }
  const files = readdirSync(dir).filter((n) => n.startsWith(prefix) && n.endsWith('.json'))
  for (const report of files.map((n) => JSON.parse(readFileSync(join(dir, n), 'utf8')))) {
    for (const file of report.testResults ?? []) {
      for (const test of file.assertionResults) {
        const group = [...test.ancestorTitles, test.title].join(' > ')
        for (const b of test.benchmarks ?? []) {
          for (const t of b.tasks) add(group, t.name, t.latency.mean, t.latency.rme)
        }
      }
    }
    for (const f of report.files ?? []) {
      for (const group of f.groups) {
        // fullName is "src/path.bench.ts > describe"; drop the file part.
        const groupName = group.fullName.split(' > ').slice(1).join(' > ')
        for (const b of group.benchmarks) add(groupName, b.name, b.mean, b.rme)
      }
    }
  }
  return results
}

function fmtTime(ns) {
  if (ns >= 1e9) return `${(ns / 1e9).toFixed(2)} s`
  if (ns >= 1e6) return `${(ns / 1e6).toFixed(2)} ms`
  if (ns >= 1e3) return `${(ns / 1e3).toFixed(2)} µs`
  return `${ns.toFixed(1)} ns`
}

const base = new Map([
  ...readCriterion(join(outDir, 'criterion'), 'bench-base-'),
  ...readVitest(outDir, 'web-base-'),
])
const pr = new Map([
  ...readCriterion(join(outDir, 'criterion'), 'bench-pr-'),
  ...readVitest(outDir, 'web-pr-'),
])

const rows = []
for (const name of new Set([...base.keys(), ...pr.keys()])) {
  const b = base.get(name)
  const p = pr.get(name)
  if (!b || !p) {
    rows.push({ name, b, p, change: null, verdict: b ? 'removed' : 'new', rank: 3 })
    continue
  }
  const change = p.point / b.point - 1
  let verdict = 'unchanged'
  let rank = 2
  if (change > threshold && p.lo > b.hi) [verdict, rank] = ['**slower**', 0]
  else if (change < -threshold && p.hi < b.lo) [verdict, rank] = ['faster', 1]
  rows.push({ name, b, p, change, verdict, rank })
}
rows.sort((x, y) => x.rank - y.rank || x.name.localeCompare(y.name))

const slower = rows.filter((r) => r.rank === 0)
const faster = rows.filter((r) => r.rank === 1)
const pct = (c) => (c === null ? '' : `${c > 0 ? '+' : ''}${(c * 100).toFixed(1)}%`)

const lines = [
  `### Benchmarks vs \`${baseLabel}\``,
  '',
  rows.length === 0
    ? 'No benchmark results found.'
    : `${slower.length} slower, ${faster.length} faster, ` +
      `${rows.length - slower.length - faster.length} unchanged or new ` +
      `(threshold ${threshold * 100}%, non-overlapping 95% CIs).`,
  '',
]
if (rows.length > 0) {
  lines.push('| Benchmark | Base | PR | Change | |', '| --- | ---: | ---: | ---: | --- |')
  for (const r of rows) {
    const cell = (e) => (e ? fmtTime(e.point) : '–')
    lines.push(`| ${r.name} | ${cell(r.b)} | ${cell(r.p)} | ${pct(r.change)} | ${r.verdict} |`)
  }
  lines.push(
    '',
    'Same runner, sides alternated, fastest round kept. Treat a lone small ' +
      'regression as a hint and rerun; a consistent one is real.'
  )
}

const report = lines.join('\n') + '\n'
process.stdout.write(report)
if (inActions && process.env.GITHUB_STEP_SUMMARY) {
  appendFileSync(process.env.GITHUB_STEP_SUMMARY, report)
}
if (inActions) {
  for (const r of slower) {
    console.log(`::warning title=Benchmark regression::${r.name} ${pct(r.change)}`)
  }
}
if (slower.length > 0 && process.env.BENCH_FAIL_ON_REGRESSION === '1') process.exit(1)
