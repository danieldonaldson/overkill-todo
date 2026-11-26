// Checks for the load tooling that need no stack (OT-013, specs/performance
// "Named load scenarios run with one command"), run by `just test-loadtest`
// and CI. k6 runs in a container, so "sends no load" means `docker` is never
// called: a stub on PATH records every call.
//
// Running a scenario against the capped stack, and the seed that gives list
// scenarios the same lists, need that stack; they are exceptions in
// docs/formal-coverage.yaml.

import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const runSh = join(here, 'run.sh')
const scenarios = readdirSync(join(here, 'scenarios'))
  .filter((f) => f.endsWith('.js'))
  .map((f) => f.slice(0, -3))

/** Runs run.sh with a stubbed `docker` that logs its calls; returns the result and the calls. */
function run(args, env = {}) {
  const bin = mkdtempSync(join(tmpdir(), 'load-test-'))
  const calls = join(bin, 'docker-calls')
  writeFileSync(join(bin, 'docker'), `#!/bin/sh\necho "$@" >> '${calls}'\nexit 1\n`)
  chmodSync(join(bin, 'docker'), 0o755)
  const started = Date.now()
  const result = spawnSync(runSh, args, {
    encoding: 'utf8',
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, LOAD_BASE_URL: '', ...env },
  })
  const elapsed = Date.now() - started
  const docker = existsSync(calls) ? readFileSync(calls, 'utf8') : ''
  rmSync(bin, { recursive: true, force: true })
  return { ...result, elapsed, docker }
}

/** A local port nothing listens on. */
async function closedPort() {
  const server = createServer()
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address()
  await new Promise((resolve) => server.close(resolve))
  return port
}

test('an unknown scenario fails at once, lists the valid names and sends no load', () => {
  const { status, stderr, elapsed, docker } = run(['no-such-scenario'])
  assert.equal(status, 1)
  assert.match(stderr, /unknown scenario 'no-such-scenario'/)
  for (const name of scenarios) assert.match(stderr, new RegExp(`\\b${name}\\b`))
  assert.ok(elapsed < 2000, `took ${elapsed} ms`)
  assert.equal(docker, '', 'docker (k6) was called')
})

test('a stack that is not running fails within 10 seconds and sends no load', async () => {
  const { status, stderr, elapsed, docker } = run(['smoke'], {
    EDGE_PORT: String(await closedPort()),
  })
  assert.equal(status, 1)
  assert.match(stderr, /the stack isn't reachable at .*\/api\/v1\/health\/ready/)
  assert.ok(elapsed < 10_000, `took ${elapsed} ms`)
  assert.equal(docker, '', 'docker (k6) was called')
})

test('a generator that cannot keep up reports the iterations not started', async () => {
  globalThis.__ENV = { LOAD_RESULT_FILE: '/results/test.json' }
  const { summaryFor } = await import('./lib/summary.js')
  const route = 'name:GET /api/v1/lists,phase:measure'
  const data = {
    state: { testRunDurationMs: 60_000 },
    metrics: {
      [`http_req_duration{${route}}`]: {
        values: { count: 100, med: 3, 'p(95)': 8, 'p(99)': 12, max: 20 },
      },
      [`http_req_failed{${route}}`]: { values: { rate: 0 } },
      'http_req_failed{phase:measure}': { values: { rate: 0 } },
      dropped_iterations: { values: { count: 42 } },
    },
  }
  const out = summaryFor('lists', 60_000)(data)
  assert.match(out.stdout, /dropped iterations 42/)
  assert.match(out.stdout, /The generator couldn't keep up: 42 planned iterations were not started/)
  assert.equal(JSON.parse(out['/results/test.json']).dropped_iterations, 42)

  const kept = summaryFor(
    'lists',
    60_000
  )({
    ...data,
    metrics: { ...data.metrics, dropped_iterations: { values: { count: 0 } } },
  })
  assert.doesNotMatch(kept.stdout, /couldn't keep up/)
})
