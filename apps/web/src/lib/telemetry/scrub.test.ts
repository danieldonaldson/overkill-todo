import { describe, expect, it } from 'vitest'

import cases from '../../../../../backend/tests/fixtures/client_scrub_cases.json'
import { positionSuffix, redactQuery, replaceEmails, scrub } from './scrub'

interface Case {
  name: string
  input: string
  max_chars: number
  expected: string
}

describe('scrub', () => {
  // The same fixture as the backend's `scrub.rs` tests: identical output.
  it.each((cases as Case[]).map((c) => [c.name, c] as const))('%s', (_name, c) => {
    expect(scrub(c.input, c.max_chars)).toBe(c.expected)
  })

  it('has the shared cases', () => {
    expect((cases as Case[]).length).toBeGreaterThanOrEqual(5)
  })

  it('changes nothing the second time', () => {
    for (const c of cases as Case[]) expect(scrub(c.expected, c.max_chars)).toBe(c.expected)
  })

  it('keeps only a trailing line and column', () => {
    expect(positionSuffix('?v=1:12:34')).toBe(':12:34')
    expect(positionSuffix('?v=1:12')).toBe(':12')
    expect(positionSuffix('?v=1')).toBe('')
    expect(positionSuffix('#a:')).toBe('')
    expect(positionSuffix('?a=1:2:3:4')).toBe(':3:4')
  })

  it('matches emails like the regex', () => {
    expect(replaceEmails('a@b.c')).toBe('a@b.c')
    expect(replaceEmails('a@b.co')).toBe('[EMAIL]')
    expect(replaceEmails('x a@b.co.')).toBe('x [EMAIL].')
    expect(replaceEmails('a@b@c.com')).toBe('a@[EMAIL]')
    expect(replaceEmails('@b.com')).toBe('@b.com')
    expect(replaceEmails('a@.com')).toBe('a@.com')
    expect(replaceEmails('one@a.io two@b.io')).toBe('[EMAIL] [EMAIL]')
    expect(replaceEmails('é a@b.io é')).toBe('é [EMAIL] é')
    const regex = /[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g
    for (const s of ['x.y+z@sub.example.co.uk!', 'a@b.c.de-f', 'q@w.e1rt.yy9', '--@--.ab']) {
      expect(replaceEmails(s)).toBe(s.replace(regex, '[EMAIL]'))
    }
  })

  it('redacts sensitive query values by key, like the backend', () => {
    expect(redactQuery('token=abc&page=2&code=123456&state=xyz&key=k&password=p&secret=s')).toBe(
      'token=[REDACTED]&page=2&code=[REDACTED]&state=[REDACTED]&key=[REDACTED]&password=[REDACTED]&secret=[REDACTED]'
    )
    expect(redactQuery('magic_token=abc')).toBe('magic_token=[REDACTED]')
    expect(redactQuery('status_code=500')).toBe('status_code=500')
    expect(redactQuery('page=2&sort=asc')).toBe('page=2&sort=asc')
  })

  it('cuts a 20,000-character message to the limit', () => {
    expect(Array.from(scrub('é'.repeat(20_000), 500))).toHaveLength(500)
  })
})
