import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { expect, test } from '@playwright/test'

// Guards the one nginx footgun that silently disarms every security header we set.
//
// `add_header` inheritance in nginx is REPLACE, not merge: a location block that
// declares ANY add_header of its own drops every add_header inherited from the
// server level. A `location ~* \.html$` that set only Cache-Control therefore
// stripped Content-Security-Policy, Permissions-Policy and Cross-Origin-Opener-Policy
// from every HTML response — exactly the documents that need them — while the
// nginx.conf still *read* as though CSP were enforced. Reviewing the file, or
// grepping it for a CSP string, does not catch this; only the block structure does.
//
// The invariant that makes it impossible: every add_header lives at server level,
// and per-path Cache-Control is computed by the $app_cache_control map instead.

const here = dirname(fileURLToPath(import.meta.url))
const nginxConf = join(here, '..', 'nginx.conf')

// Strip `#` comments so a commented-out add_header never trips the scan. Not
// quote-aware, which is fine: nginx.conf has no `#` inside a quoted value.
function stripComments(conf: string): string {
  return conf
    .split('\n')
    .map((line) => line.replace(/#.*$/, ''))
    .join('\n')
}

// Every `location <match> { ... }` body, brace-matched so nested blocks are included.
function locationBodies(conf: string): { header: string; body: string }[] {
  const out: { header: string; body: string }[] = []
  const re = /\blocation\b([^{]*)\{/g
  let m: RegExpExecArray | null
  while ((m = re.exec(conf)) !== null) {
    let depth = 1
    let i = m.index + m[0].length
    const start = i
    while (i < conf.length && depth > 0) {
      if (conf[i] === '{') depth++
      else if (conf[i] === '}') depth--
      i++
    }
    expect(depth, `unbalanced braces after "location${m[1]}"`).toBe(0)
    out.push({ header: `location${m[1].trim()}`, body: conf.slice(start, i - 1) })
  }
  return out
}

test('no location block declares add_header (it would shadow the server-level ones)', () => {
  const conf = stripComments(readFileSync(nginxConf, 'utf8'))
  const blocks = locationBodies(conf)
  expect(blocks.length, 'nginx.conf defines location blocks').toBeGreaterThan(0)

  const offenders = blocks
    .filter((b) => /\badd_header\b/.test(b.body))
    .map((b) => b.header)

  expect(
    offenders,
    'these location blocks declare add_header, which silently drops every ' +
      'server-level security header for the paths they match; express caching ' +
      'through the $app_cache_control map instead',
  ).toEqual([])
})

test('the security headers are declared at server level', () => {
  const conf = stripComments(readFileSync(nginxConf, 'utf8'))
  for (const header of [
    'Content-Security-Policy',
    'Permissions-Policy',
    'Cross-Origin-Opener-Policy',
    'X-Frame-Options',
    'X-Content-Type-Options',
    'Referrer-Policy',
    'Strict-Transport-Security',
  ]) {
    expect(conf, `${header} is set in nginx.conf`).toContain(`add_header ${header} `)
  }
})
