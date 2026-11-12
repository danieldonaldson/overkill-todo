import { beforeEach, describe, expect, it } from 'vitest'
import { clearSessionUser, getSessionUser, setSessionUser } from './session'
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
