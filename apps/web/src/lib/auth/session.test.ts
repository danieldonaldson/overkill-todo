import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  clearSessionUser,
  confirmSessionUser,
  getSessionUser,
  isSessionUserConfirmed,
  setSessionUser,
} from './session'
import { clearTokenExpiry, setTokenExpiry } from './token'
import type { User } from '../types'

const STORAGE_KEY = 'overkill-todo-auth-user'

const user: User = {
  id: 'user-1',
  email: 'test@example.com',
  first_name: 'Test',
  last_name: 'User',
  email_verified: true,
  has_password: true,
  oauth_providers: [],
  created_at: '2026-01-01T00:00:00Z',
  role: 'user',
}

beforeEach(() => {
  sessionStorage.clear()
  clearTokenExpiry()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('session user storage', () => {
  it('round-trips a cached user', () => {
    setSessionUser(user)
    expect(getSessionUser()).toEqual(user)
  })

  it('returns null when no user is cached', () => {
    expect(getSessionUser()).toBeNull()
  })

  it('returns null when the stored value is malformed JSON', () => {
    sessionStorage.setItem(STORAGE_KEY, '{not valid json')
    expect(getSessionUser()).toBeNull()
  })

  it('clears the cached user', () => {
    setSessionUser(user)
    clearSessionUser()
    expect(getSessionUser()).toBeNull()
  })
})

describe('confirmed window', () => {
  const NOW = 1_800_000_000

  beforeEach(() => {
    vi.useFakeTimers({ now: NOW * 1000 })
  })

  it('trusts a confirmed user until the access token it was confirmed under expires', () => {
    setTokenExpiry(NOW + 900)
    confirmSessionUser(user)
    expect(getSessionUser()).toEqual(user)
    expect(isSessionUserConfirmed()).toBe(true)

    vi.setSystemTime((NOW + 900) * 1000)
    expect(isSessionUserConfirmed()).toBe(false)
  })

  it('keeps the window to the confirming token, even after a refresh extends the hint', () => {
    setTokenExpiry(NOW + 900)
    confirmSessionUser(user)
    setTokenExpiry(NOW + 1800)

    vi.setSystemTime((NOW + 1000) * 1000)
    expect(isSessionUserConfirmed()).toBe(false)
  })

  it('does not count a user cached without /auth/me as confirmed', () => {
    setTokenExpiry(NOW + 900)
    setSessionUser(user)
    expect(isSessionUserConfirmed()).toBe(false)
  })

  it('leaves the window alone when the cached user is updated after an action', () => {
    setTokenExpiry(NOW + 900)
    confirmSessionUser(user)
    setSessionUser({ ...user, has_password: false })
    expect(isSessionUserConfirmed()).toBe(true)
  })

  it('stops trusting the user once the session hint is gone', () => {
    setTokenExpiry(NOW + 900)
    confirmSessionUser(user)
    clearTokenExpiry()
    expect(isSessionUserConfirmed()).toBe(false)
  })

  it('clearing the user clears the window', () => {
    setTokenExpiry(NOW + 900)
    confirmSessionUser(user)
    clearSessionUser()
    setSessionUser(user)
    expect(isSessionUserConfirmed()).toBe(false)
  })
})
