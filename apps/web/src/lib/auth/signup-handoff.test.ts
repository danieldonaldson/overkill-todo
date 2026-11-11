import { afterEach, describe, expect, it } from 'vitest'

import { clearSignupHandoff, readSignupHandoff, saveSignupHandoff } from './signup-handoff'

describe('signup handoff', () => {
  afterEach(() => sessionStorage.clear())

  it('round-trips a saved handoff', () => {
    saveSignupHandoff({ token: 'signup-token', email: 'a@example.com' })
    expect(readSignupHandoff()).toEqual({ token: 'signup-token', email: 'a@example.com' })
  })

  it('returns null when nothing was saved', () => {
    expect(readSignupHandoff()).toBeNull()
  })

  it('returns null for malformed storage', () => {
    sessionStorage.setItem('signup_handoff', '{not json')
    expect(readSignupHandoff()).toBeNull()
    sessionStorage.setItem('signup_handoff', JSON.stringify({ token: 1 }))
    expect(readSignupHandoff()).toBeNull()
  })

  it('clears the handoff', () => {
    saveSignupHandoff({ token: 't', email: 'e@example.com' })
    clearSignupHandoff()
    expect(readSignupHandoff()).toBeNull()
  })
})
