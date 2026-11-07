// The deployed environment under test. Set SMOKE_BASE_URL to the deployment's
// origin (the app and /api/v1 share it); the default is a placeholder.
export const APP_BASE_URL = (process.env.SMOKE_BASE_URL ?? 'https://app.example.com').replace(
  /\/$/,
  '',
)


// Header names are case-insensitive on the wire and HTTP/2 lowercases them;
// Playwright's headers() already returns them lowercased, but read through this
// so a test never depends on that.
export function header(headers: Record<string, string>, name: string): string | undefined {
  return headers[name.toLowerCase()]
}

// Every security header the edge + app are expected to set on a response that a
// browser will treat as a document or as executable content. The nginx bug this
// suite exists to catch removed exactly these, one location block at a time.
export const REQUIRED_SECURITY_HEADERS = [
  'content-security-policy',
  'permissions-policy',
  'cross-origin-opener-policy',
  'strict-transport-security',
  'x-content-type-options',
  'x-frame-options',
  'referrer-policy',
] as const

// Parse a CSP header into directive → source list.
export function parseCsp(csp: string): Map<string, string[]> {
  const out = new Map<string, string[]>()
  for (const part of csp.split(';')) {
    const [name, ...sources] = part.trim().split(/\s+/)
    if (name) out.set(name.toLowerCase(), sources)
  }
  return out
}
