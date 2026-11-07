import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { expect, test } from '@playwright/test'

// Guards the structure that keeps every security header on every response.
//
// History: under nginx, a location block that declared any add_header of its own
// silently dropped every server-level one, and production served HTML with no CSP while
// the config still read as though it were enforced. Caddy doesn't inherit that way, but
// it has its own two ways to get the same outcome:
//   1. Moving the header block inside a `route`/`handle`, where directives run in the
//      order written — after a terminal handler (respond, file_server) it never runs.
//      At the site's top level Caddy sorts `header` ahead of every handler.
//   2. A second `header` directive naming one of these fields overwrites or deletes it
//      for whatever paths that directive matches.
// The smoke suite checks the bytes production returns; this checks the file.

const here = dirname(fileURLToPath(import.meta.url))
const caddyfile = join(here, '..', 'Caddyfile')

const SECURITY_HEADERS = [
  'Content-Security-Policy',
  'Permissions-Policy',
  'Cross-Origin-Opener-Policy',
  'X-Frame-Options',
  'X-Content-Type-Options',
  'Referrer-Policy',
  'Strict-Transport-Security',
]

// `#` starts a comment unless it's inside a quoted value. Our values contain no `#`,
// so a plain strip is enough — but keep quoted text intact just in case.
function stripComment(line: string): string {
  let inQuote = false
  for (let i = 0; i < line.length; i++) {
    if (line[i] === '"' && line[i - 1] !== '\\') inQuote = !inQuote
    else if (line[i] === '#' && !inQuote) return line.slice(0, i)
  }
  return line
}

type Line = { text: string; depth: number }

// Each non-empty line with its brace depth (0 = global options / site address).
function lines(): Line[] {
  const out: Line[] = []
  let depth = 0
  for (const raw of readFileSync(caddyfile, 'utf8').split('\n')) {
    const text = stripComment(raw).trim()
    if (!text) continue
    const opensAfter = text.endsWith('{')
    if (text.startsWith('}')) depth--
    out.push({ text, depth })
    if (opensAfter) depth++
  }
  expect(depth, 'Caddyfile braces are balanced').toBe(0)
  return out
}

// The unmatched, site-level `header {` block — the one place these fields may be set.
function siteHeaderBlock(all: Line[]): { start: number; end: number } {
  const start = all.findIndex((l) => l.depth === 1 && l.text === 'header {')
  expect(
    start,
    'the site block has an unmatched `header {` block at its top level'
  ).toBeGreaterThan(-1)
  const end = all.findIndex((l, i) => i > start && l.depth === 1 && l.text === '}')
  return { start, end }
}

function fieldOf(directiveLine: string): string | undefined {
  // `header [matcher] [+|-|>|?]Field [value]` — the matcher, if any, starts with @, / or *.
  const words = directiveLine.split(/\s+/).slice(1)
  const field = words.find((w) => !/^[@/*]/.test(w) && w !== '{')
  return field?.replace(/^[-+>?]/, '')
}

test('the security headers are set in the site-level, unmatched header block', () => {
  const all = lines()
  const { start, end } = siteHeaderBlock(all)
  const declared = all.slice(start + 1, end).map((l) => l.text.split(/\s+/)[0])
  for (const header of SECURITY_HEADERS) {
    expect(declared, `${header} is set in the site-level header block`).toContain(header)
  }
})

test('no other header directive overwrites or deletes a security header', () => {
  const all = lines()
  const { start, end } = siteHeaderBlock(all)
  const offenders: string[] = []
  all.forEach((l, i) => {
    if (i >= start && i <= end) return
    if (!/^header\b/.test(l.text)) return
    const field = fieldOf(l.text)
    if (field && SECURITY_HEADERS.some((h) => h.toLowerCase() === field.toLowerCase())) {
      offenders.push(l.text)
    }
    // A block form (`header [matcher] {`) — check every field inside it too.
    if (l.text.endsWith('{')) {
      for (let j = i + 1; j < all.length && all[j].depth > l.depth; j++) {
        const inner = all[j].text.split(/\s+/)[0].replace(/^[-+>?]/, '')
        if (SECURITY_HEADERS.some((h) => h.toLowerCase() === inner.toLowerCase())) {
          offenders.push(`${l.text} … ${all[j].text}`)
        }
      }
    }
  })
  expect(
    offenders,
    'these directives replace or strip a security header for the paths they match; ' +
      'set security headers only in the site-level header block'
  ).toEqual([])
})

test('the admin API is disabled', () => {
  const globals = lines().filter((l) => l.depth === 1)
  expect(globals.map((l) => l.text)).toContain('admin off')
})
