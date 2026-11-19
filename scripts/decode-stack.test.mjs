#!/usr/bin/env node
// Check for `just decode-stack` (OT-006 task 1.12), run by `just test-web`:
// build a fixture the way the site is built (Vite, minified, hidden source
// maps), throw from it, and decode the stack. The top frame must name the
// fixture's source file and the line that threw.

import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync, mkdirSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '..')
// Vite as the site uses it: Astro's own dependency.
const fromWeb = createRequire(join(repo, 'apps', 'web', 'package.json'))
const vite = await import(
  pathToFileURL(createRequire(fromWeb.resolve('astro/package.json')).resolve('vite')).href
)

const THROW_LINE = 7
const source = [
  '// Fixture for decode-stack.test.mjs.',
  'export function render(items) {',
  '  let total = 0',
  '  for (const item of items) {',
  '    total += item.price',
  '  }',
  "  if (total > 10) throw new Error('total too high: ' + total)",
  '  return total',
  '}',
  '',
].join('\n')

const dir = mkdtempSync(join(tmpdir(), 'decode-stack-'))
let failed = false
try {
  mkdirSync(join(dir, 'src'))
  writeFileSync(join(dir, 'src', 'render.js'), source)
  await vite.build({
    root: dir,
    logLevel: 'silent',
    configFile: false,
    build: {
      outDir: join(dir, 'dist'),
      sourcemap: 'hidden',
      minify: true,
      lib: { entry: join(dir, 'src', 'render.js'), formats: ['es'], fileName: 'render.min' },
    },
  })
  const built = readdirSync(join(dir, 'dist')).find((f) => /\.m?js$/.test(f))
  const code = readFileSync(join(dir, 'dist', built), 'utf8')
  if (code.includes('sourceMappingURL')) throw new Error('hidden maps must not be referenced')

  const { render } = await import(pathToFileURL(join(dir, 'dist', built)).href)
  let stack
  try {
    render([{ price: 6 }, { price: 6 }])
  } catch (error) {
    stack = error.stack
  }
  if (!stack) throw new Error('the fixture did not throw')

  const decoded = execFileSync(
    process.execPath,
    [join(repo, 'scripts', 'decode-stack.mjs'), '--maps', join(dir, 'dist')],
    { input: stack, encoding: 'utf8' }
  )
  const top = decoded.split('\n').find((l) => l.trim().startsWith('at '))
  const expected = `src/render.js:${THROW_LINE}:`
  if (!top?.includes(expected)) {
    failed = true
    console.error(
      `decode-stack: top frame should name ${expected}\nstack:\n${stack}\ndecoded:\n${decoded}`
    )
  } else {
    console.log(`decode-stack: ok (${top.trim()})`)
  }
} finally {
  rmSync(dir, { recursive: true, force: true })
}
process.exit(failed ? 1 : 0)
