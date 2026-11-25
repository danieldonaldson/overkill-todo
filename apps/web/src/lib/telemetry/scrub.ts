// Scrubbing for browser report text (OT-006 Decision 4), applied before a
// report leaves the browser. The server applies the same rules again and its
// copy (`backend/crates/application/src/use_cases/telemetry/scrub.rs`) is the
// authoritative one. Both run every case in
// `backend/tests/fixtures/client_scrub_cases.json`, so a change here needs the
// same change there, and a new case in the fixture.
//
// In order:
// 1. Addresses: a word that looks like an address (contains `://` or starts
//    with `/`) loses everything from its first `?` or `#`, except a trailing
//    `:line` or `:line:column`, so stack frames keep their position. Words end
//    at whitespace and at " ' < > ( ) [ ] { } and backticks.
// 2. Emails become `[EMAIL]`.
// 3. List IDs in a page address, and UUIDs after `/lists/` or `/todos/` in an
//    API path, become `[ID]` (OT-013 design D9).
// 4. `key=value` pairs left in the text have the value of a sensitive key
//    replaced (the backend's `redact_query`).
// 5. Length: cut to `maxChars` characters (code points, as in Rust).

export const EMAIL_PLACEHOLDER = '[EMAIL]'
export const REDACTED = '[REDACTED]'
export const PATH_ID_PLACEHOLDER = '[ID]'

export const MAX_MESSAGE_CHARS = 500
export const MAX_STACK_CHARS = 4000
export const MAX_SOURCE_CHARS = 500

/** Apply every rule above to `input`. */
export function scrub(input: string, maxChars: number): string {
  return truncateChars(
    redactKeyValues(replaceEmails(replacePathIds(stripAddressQueries(input)))),
    maxChars
  )
}

/** Rule 3: list addresses and API ids carry no identity in a report. */
function replacePathIds(input: string): string {
  return replaceUuidAfter(replaceUuidAfter(replaceDashboardListIds(input), '/lists/'), '/todos/')
}

function replaceDashboardListIds(input: string): string {
  const marker = '/dashboard/lists/'
  let out = ''
  let from = 0
  for (;;) {
    const at = input.indexOf(marker, from)
    if (at === -1) return out + input.slice(from)
    out += input.slice(from, at + marker.length)
    const start = at + marker.length
    if (input.startsWith(PATH_ID_PLACEHOLDER, start)) {
      out += PATH_ID_PLACEHOLDER
      from = start + PATH_ID_PLACEHOLDER.length
      continue
    }
    let end = start
    while (end < input.length && !endsSegment(input[end])) end++
    out += PATH_ID_PLACEHOLDER
    from = end
  }
}

function replaceUuidAfter(input: string, marker: string): string {
  let out = ''
  let from = 0
  for (;;) {
    const at = input.indexOf(marker, from)
    if (at === -1) return out + input.slice(from)
    const start = at + marker.length
    let end = start
    while (end < input.length && !endsSegment(input[end])) end++
    if (isUuid(input.slice(start, end))) {
      out += input.slice(from, start) + PATH_ID_PLACEHOLDER
      from = end
    } else {
      out += input.slice(from, start)
      from = start
    }
  }
}

const isUuid = (value: string) =>
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)

function endsSegment(c: string): boolean {
  return c === '/' || c === '?' || c === '#' || isWhitespace(c) || WORD_ENDS.has(c)
}

// Rust's `char::is_whitespace` (Unicode White_Space), which differs from
// JavaScript's `\s` in U+0085 and U+FEFF.
function isWhitespace(c: string): boolean {
  const code = c.codePointAt(0) ?? 0
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  )
}

const WORD_ENDS = new Set(['"', "'", '<', '>', '(', ')', '[', ']', '{', '}', '`'])

function endsWord(c: string): boolean {
  return isWhitespace(c) || WORD_ENDS.has(c)
}

function stripAddressQueries(input: string): string {
  let out = ''
  let word = ''
  for (const c of input) {
    if (endsWord(c)) {
      out += stripWordQuery(word) + c
      word = ''
    } else {
      word += c
    }
  }
  return out + stripWordQuery(word)
}

function stripWordQuery(word: string): string {
  const scheme = word.indexOf('://')
  let pathStart: number
  if (scheme !== -1) pathStart = scheme + 3
  else if (word.startsWith('/')) pathStart = 0
  else return word
  const rest = word.slice(pathStart)
  const offsets = [rest.indexOf('?'), rest.indexOf('#')].filter((i) => i !== -1)
  if (offsets.length === 0) return word
  const cut = pathStart + Math.min(...offsets)
  return word.slice(0, cut) + positionSuffix(word.slice(cut))
}

const isDigit = (c: string) => c >= '0' && c <= '9'

/** A trailing `:digits` or `:digits:digits` of `tail`, or `''`. */
export function positionSuffix(tail: string): string {
  let suffixStart = tail.length
  for (let round = 0; round < 2; round++) {
    let digitsStart = suffixStart
    while (digitsStart > 0 && isDigit(tail[digitsStart - 1])) digitsStart--
    if (digitsStart === suffixStart || tail[digitsStart - 1] !== ':') break
    suffixStart = digitsStart - 1
  }
  return tail.slice(suffixStart)
}

const isAsciiAlpha = (c: string) => (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z')
const isAsciiAlphanumeric = (c: string) => isAsciiAlpha(c) || isDigit(c)
const isLocalChar = (c: string) => isAsciiAlphanumeric(c) || '._%+-'.includes(c)
const isDomainChar = (c: string) => isAsciiAlphanumeric(c) || c === '.' || c === '-'

/** Rule 2: the matches of `/[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g`. */
export function replaceEmails(input: string): string {
  const chars = Array.from(input)
  let out = ''
  let copied = 0
  let at = 0
  while (at < chars.length) {
    if (chars[at] !== '@') {
      at++
      continue
    }
    let start = at
    while (start > copied && isLocalChar(chars[start - 1])) start--
    let runEnd = at + 1
    while (runEnd < chars.length && isDomainChar(chars[runEnd])) runEnd++
    const len = start < at ? domainEnd(chars.slice(at + 1, runEnd)) : undefined
    if (len === undefined) {
      at++
      continue
    }
    out += chars.slice(copied, start).join('') + EMAIL_PLACEHOLDER
    copied = at + 1 + len
    at = copied
  }
  return out + chars.slice(copied).join('')
}

// Length of the longest prefix of `run` that is `X.L`, with `X` non-empty and
// `L` two or more ASCII letters.
function domainEnd(run: string[]): number | undefined {
  for (let end = run.length; end >= 1; end--) {
    let letters = 0
    while (letters < end && isAsciiAlpha(run[end - 1 - letters])) letters++
    if (letters >= 2 && end > letters + 1 && run[end - letters - 1] === '.') return end
  }
  return undefined
}

// The backend's field-name rules (`shared/redact.rs`).
const SENSITIVE_FIELD_PARTS = [
  'password',
  'passwd',
  'secret',
  'token',
  'code',
  'otp',
  'session',
  'cookie',
  'authorization',
  'csrf',
]
const ALLOWED_FIELDS = ['status_code', 'http.response.status_code', 'otel.status_code']
const ALLOWED_PREFIXES = ['code.']
const SENSITIVE_QUERY_PARAMS = ['token', 'code', 'password', 'secret', 'key', 'state']

// ASCII-only, like Rust's `to_ascii_lowercase` (`toLowerCase` would also fold
// characters such as the Kelvin sign into `k`).
const asciiLower = (s: string) => s.replace(/[A-Z]/g, (c) => c.toLowerCase())

function isSensitiveField(name: string): boolean {
  const lower = asciiLower(name)
  if (ALLOWED_FIELDS.includes(lower) || ALLOWED_PREFIXES.some((p) => lower.startsWith(p))) {
    return false
  }
  return SENSITIVE_FIELD_PARTS.some((part) => lower.includes(part))
}

function isSensitiveQueryParam(name: string): boolean {
  return SENSITIVE_QUERY_PARAMS.includes(asciiLower(name)) || isSensitiveField(name)
}

/** The backend's `redact_query`: `a=1&token=x` → `a=1&token=[REDACTED]`. */
export function redactQuery(query: string): string {
  return query
    .split('&')
    .map((pair) => {
      const eq = pair.indexOf('=')
      if (eq === -1) return pair
      const key = pair.slice(0, eq)
      return isSensitiveQueryParam(key) ? `${key}=${REDACTED}` : pair
    })
    .join('&')
}

function redactKeyValues(input: string): string {
  let out = ''
  let word = ''
  const flush = () => (word.includes('=') ? redactQuery(word) : word)
  for (const c of input) {
    if (isWhitespace(c)) {
      out += flush() + c
      word = ''
    } else {
      word += c
    }
  }
  return out + flush()
}

function truncateChars(input: string, maxChars: number): string {
  const chars = Array.from(input)
  return chars.length > maxChars ? chars.slice(0, maxChars).join('') : input
}
