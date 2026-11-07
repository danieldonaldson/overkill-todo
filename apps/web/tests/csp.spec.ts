import { createHash } from 'node:crypto'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { expect, test } from '@playwright/test'

// Guards the CSP hardening: script-src must not fall back to
// 'unsafe-inline', and every inline <script> the build still emits — chiefly
// Astro's island-hydration bootstrap — must be pinned by a SHA-256 hash in
// nginx.conf. An Astro upgrade that changes that snippet fails this test with
// the new hash to paste in, instead of silently breaking hydration in prod.

const here = dirname(fileURLToPath(import.meta.url))
const distDir = join(here, '..', 'dist')
const nginxConf = join(here, '..', 'nginx.conf')

function readScriptSrc(): string {
  const conf = readFileSync(nginxConf, 'utf8')
  const csp = conf.match(/Content-Security-Policy\s+"([^"]*)"/)?.[1]
  expect(csp, 'nginx.conf has a Content-Security-Policy header').toBeTruthy()
  const scriptSrc = csp!.split(';').find((d) => d.trim().startsWith('script-src'))
  expect(scriptSrc, 'CSP has a script-src directive').toBeTruthy()
  return scriptSrc!.trim()
}

function walkHtml(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) return walkHtml(full)
    return entry.name.endsWith('.html') ? [full] : []
  })
}

// Inline scripts only: <script> ... </script> with no src= attribute.
function inlineScripts(html: string): string[] {
  const out: string[] = []
  const re = /<script(\b[^>]*)>([\s\S]*?)<\/script>/g
  let m: RegExpExecArray | null
  while ((m = re.exec(html)) !== null) {
    const attrs = m[1]
    if (!/\bsrc\s*=/.test(attrs)) out.push(m[2])
  }
  return out
}

function sha256(content: string): string {
  return `sha256-${createHash('sha256').update(content, 'utf8').digest('base64')}`
}

test('script-src does not allow unsafe-inline', () => {
  expect(readScriptSrc()).not.toContain("'unsafe-inline'")
})

test('every inline script in the build is pinned by a CSP hash', () => {
  const scriptSrc = readScriptSrc()
  const htmlFiles = walkHtml(distDir)
  expect(htmlFiles.length, 'build produced HTML pages').toBeGreaterThan(0)

  const unpinned = new Set<string>()
  for (const file of htmlFiles) {
    for (const script of inlineScripts(readFileSync(file, 'utf8'))) {
      const hash = sha256(script)
      if (!scriptSrc.includes(hash)) unpinned.add(hash)
    }
  }

  expect(
    [...unpinned],
    'inline scripts missing from script-src; add these hashes to nginx.conf',
  ).toEqual([])
})

// ── Third-party origins the app actually depends on ────────────────────────────
//
// A CSP that is too TIGHT breaks the product silently, and does it in production
// rather than in review — a directive that is merely absent looks like nothing at all.
// (Enforcing CSP can silently break an inline payment checkout this way.) So pin the integrations we know we have. Each entry says which directive the origin
// must appear in, and why — so a future edit that trims one has to argue with a name.
const REQUIRED_ORIGINS: { origin: string; directives: string[]; why: string }[] = [
  {
    origin: 'https://challenges.cloudflare.com',
    directives: ['script-src', 'frame-src'],
    why: 'Turnstile on login/register: api.js plus the widget iframe',
  },
]

function directive(csp: string, name: string): string {
  const found = csp
    .split(';')
    .map((d) => d.trim())
    .find((d) => d === name || d.startsWith(`${name} `))
  expect(found, `CSP declares a ${name} directive`).toBeTruthy()
  return found!
}

test('every third-party origin the app depends on is allowed by the CSP', () => {
  const conf = readFileSync(nginxConf, 'utf8')
  const csp = conf.match(/Content-Security-Policy\s+"([^"]*)"/)?.[1]
  expect(csp, 'nginx.conf has a Content-Security-Policy header').toBeTruthy()

  const missing: string[] = []
  for (const { origin, directives, why } of REQUIRED_ORIGINS) {
    for (const name of directives) {
      if (!directive(csp!, name).includes(origin)) {
        missing.push(`${name} is missing ${origin} — ${why}`)
      }
    }
  }
  expect(missing, 'removing these breaks a user-facing flow in production').toEqual([])
})
