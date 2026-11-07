import { describe, expect, it } from 'vitest'
import {
  email,
  maxLength,
  minLength,
  pattern,
  required,
  validate,
  validateAll,
} from './validation'

describe('required', () => {
  it('returns null for non-empty values', () => {
    expect(required()('hello')).toBeNull()
  })

  it('returns an error for empty strings', () => {
    expect(required()('')).toBe('This field is required')
  })

  it('treats whitespace-only values as empty', () => {
    expect(required()('   ')).toBe('This field is required')
  })

  it('uses a custom message', () => {
    expect(required('Name needed')('')).toBe('Name needed')
  })
})

describe('email', () => {
  it('accepts a valid address', () => {
    expect(email()('user@example.com')).toBeNull()
  })

  it('rejects an address with no domain', () => {
    expect(email()('user@')).toBe('Please enter a valid email address')
  })

  it('rejects an address with no TLD', () => {
    expect(email()('user@example')).toBe('Please enter a valid email address')
  })

  it('rejects an address with whitespace', () => {
    expect(email()('user @example.com')).toBe('Please enter a valid email address')
  })

  it('uses a custom message', () => {
    expect(email('Bad email')('nope')).toBe('Bad email')
  })
})

describe('minLength', () => {
  it('accepts values at the boundary', () => {
    expect(minLength(3)('abc')).toBeNull()
  })

  it('rejects shorter values with a default message', () => {
    expect(minLength(3)('ab')).toBe('Must be at least 3 characters')
  })

  it('uses a custom message', () => {
    expect(minLength(3, 'Too short')('a')).toBe('Too short')
  })
})

describe('maxLength', () => {
  it('accepts values at the boundary', () => {
    expect(maxLength(3)('abc')).toBeNull()
  })

  it('rejects longer values with a default message', () => {
    expect(maxLength(3)('abcd')).toBe('Must be at most 3 characters')
  })

  it('uses a custom message', () => {
    expect(maxLength(3, 'Too long')('abcd')).toBe('Too long')
  })
})

describe('pattern', () => {
  it('accepts values matching the regex', () => {
    expect(pattern(/^\d+$/, 'Digits only')('123')).toBeNull()
  })

  it('rejects values not matching the regex', () => {
    expect(pattern(/^\d+$/, 'Digits only')('12a')).toBe('Digits only')
  })
})

describe('validate', () => {
  it('returns null when all rules pass', () => {
    expect(validate('user@example.com', [required(), email()])).toBeNull()
  })

  it('returns the first failing rule error in order', () => {
    expect(validate('', [required(), email()])).toBe('This field is required')
  })

  it('runs later rules when earlier ones pass', () => {
    expect(validate('notanemail', [required(), email()])).toBe(
      'Please enter a valid email address'
    )
  })
})

describe('validateAll', () => {
  it('returns no errors when every field passes', () => {
    const errors = validateAll(
      { email: 'user@example.com', password: 'longenough' },
      { email: [required(), email()], password: [minLength(8)] }
    )
    expect(errors).toEqual({})
  })

  it('collects errors keyed by field', () => {
    const errors = validateAll(
      { email: 'bad', password: 'x' },
      { email: [email()], password: [minLength(8)] }
    )
    expect(errors).toEqual({
      email: 'Please enter a valid email address',
      password: 'Must be at least 8 characters',
    })
  })

  it('skips fields with no rules', () => {
    const errors = validateAll(
      { email: 'bad', name: 'whatever' },
      { email: [email()] }
    )
    expect(errors).toEqual({ email: 'Please enter a valid email address' })
  })
})
