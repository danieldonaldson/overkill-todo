import { describe, expect, it } from 'vitest'

import { resolvePublicValue } from './config'

// How a PUBLIC_* value such as UMAMI_WEBSITE_ID (the OT-005 analytics flag)
// is resolved from the rendered /config.js and the build-time env.
describe('resolvePublicValue', () => {
  const id = '4b0b8e0a-7a4c-4f53-9b1e-2f1d1c6a0e11'

  it('uses the rendered runtime value when set', () => {
    expect(resolvePublicValue(id, undefined)).toBe(id)
    expect(resolvePublicValue(id, 'build-value')).toBe(id)
  })

  it('is empty (off) when unset everywhere', () => {
    expect(resolvePublicValue('', undefined)).toBe('')
    expect(resolvePublicValue(undefined, undefined)).toBe('')
    expect(resolvePublicValue(undefined, '')).toBe('')
  })

  it('treats an unrendered placeholder as unset', () => {
    // The dev server serves public/config.js verbatim.
    expect(resolvePublicValue('${PUBLIC_UMAMI_WEBSITE_ID}', undefined)).toBe('')
  })

  it('falls back to the build-time env behind a placeholder or empty value', () => {
    expect(resolvePublicValue('${PUBLIC_UMAMI_WEBSITE_ID}', 'build-value')).toBe('build-value')
    expect(resolvePublicValue('', 'build-value')).toBe('build-value')
  })
})

describe('UMAMI_WEBSITE_ID', () => {
  it('is off in the test environment', async () => {
    const { UMAMI_WEBSITE_ID } = await import('./config')
    expect(UMAMI_WEBSITE_ID).toBe('')
  })
})
