import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest'

vi.mock('../api', () => ({
  api: {
    auth: {
      me: vi.fn(),
      logout: vi.fn(),
    },
  },
}))

vi.mock('../navigation', () => ({
  navigateReplace: vi.fn(),
}))

import { api } from '../api'
import { ApiErrorException } from '../api/errors'
import { navigateReplace } from '../navigation'
import type { User } from '../types'
import { resetSession, session, SIGN_OUT_TIMEOUT_MS } from './session.svelte'
import { getSessionUser, setSessionUser } from './session'
import { getTokenExpiry, setTokenExpiry } from './token'

const me = api.auth.me as Mock
const logout = api.auth.logout as Mock

const user: User = {
  id: 'user-1',
  email: 'test+1@example.com',
  first_name: 'Test',
  last_name: 'User',
  email_verified: true,
  has_password: true,
  oauth_providers: [],
  created_at: '2026-01-01T00:00:00Z',
  role: 'user',
}

/** A promise plus its resolve/reject, for controlling when a mock answers. */
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

beforeEach(() => {
  setTokenExpiry(1_700_000_000)
  resetSession()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('session store initial state', () => {
  it('starts from the cached user, unconfirmed', () => {
    setSessionUser(user)
    resetSession()

    expect(session.user).toEqual(user)
    expect(session.status).toBe('unconfirmed')
  })

  it('starts with no user when nothing is cached', () => {
    expect(session.user).toBeNull()
  })
})

describe('session.init (session-hint backstop)', () => {
  it('sends a tab with no session hint to /login without checking the server', () => {
    localStorage.clear()

    expect(session.init()).toBe(false)
    expect(navigateReplace).toHaveBeenCalledWith('/login')
    expect(me).not.toHaveBeenCalled()
  })

  it('starts the background check when the hint is present', () => {
    me.mockResolvedValue(user)

    expect(session.init()).toBe(true)
    expect(navigateReplace).not.toHaveBeenCalled()
    expect(me).toHaveBeenCalledTimes(1)
  })
})

describe('session.revalidate', () => {
  it('makes one /auth/me call for concurrent and repeated callers', async () => {
    const answer = deferred<User>()
    me.mockReturnValue(answer.promise)

    const first = session.revalidate()
    const second = session.revalidate()
    answer.resolve(user)
    await Promise.all([first, second])
    await session.revalidate()

    expect(me).toHaveBeenCalledTimes(1)
  })

  it('sets the user, confirms, and writes through to the cache on success', async () => {
    setSessionUser({ ...user, has_password: false })
    resetSession()
    me.mockResolvedValue(user)

    await session.revalidate()

    expect(session.user).toEqual(user)
    expect(session.status).toBe('confirmed')
    expect(getSessionUser()).toEqual(user)
    expect(navigateReplace).not.toHaveBeenCalled()
  })

  it('sends an unverified account to email verification', async () => {
    me.mockResolvedValue({ ...user, email_verified: false })

    await session.revalidate()

    expect(navigateReplace).toHaveBeenCalledWith('/verify-email?email=test%2B1%40example.com')
  })

  it.each([
    ['a network failure', new TypeError('Failed to fetch')],
    ['a 500', new ApiErrorException({ message: 'Boom' }, { status: 500 })],
    ['a 503', new ApiErrorException(null, { status: 503 })],
  ])('marks the session unreachable on %s, keeping the user and the page', async (_, err) => {
    setSessionUser(user)
    resetSession()
    me.mockRejectedValue(err)

    await session.revalidate()

    expect(session.status).toBe('unreachable')
    expect(session.user).toEqual(user)
    expect(getSessionUser()).toEqual(user)
    expect(navigateReplace).not.toHaveBeenCalled()
  })

  it('leaves the status alone for errors HttpClient has already handled', async () => {
    me.mockRejectedValue(new Error('Session expired'))

    await session.revalidate()

    expect(session.status).toBe('unconfirmed')
    expect(navigateReplace).not.toHaveBeenCalled()
  })
})

describe('session.revalidate with a confirmed cache', () => {
  const NOW = 1_800_000_000

  beforeEach(() => {
    vi.useFakeTimers({ now: NOW * 1000 })
    setTokenExpiry(NOW + 900)
  })

  it('confirms the account on the first load and trusts it on the next', async () => {
    me.mockResolvedValue(user)
    await session.revalidate()
    expect(me).toHaveBeenCalledTimes(1)

    resetSession() // a reload: new document, same tab
    await session.revalidate()

    expect(me).toHaveBeenCalledTimes(1)
    expect(session.user).toEqual(user)
    expect(session.status).toBe('confirmed')
  })

  it('checks again once the access token the account was confirmed under has expired', async () => {
    me.mockResolvedValue(user)
    await session.revalidate()

    vi.setSystemTime((NOW + 901) * 1000)
    resetSession()
    await session.revalidate()

    expect(me).toHaveBeenCalledTimes(2)
  })

  it('always checks an unverified account', async () => {
    me.mockResolvedValue({ ...user, email_verified: false })
    await session.revalidate()

    resetSession()
    await session.revalidate()

    expect(me).toHaveBeenCalledTimes(2)
  })

  it('retry asks the server even when the cache is trusted', async () => {
    me.mockResolvedValue(user)
    await session.revalidate()
    resetSession()
    await session.revalidate()

    await session.retry()

    expect(me).toHaveBeenCalledTimes(2)
  })
})

describe('session.retry', () => {
  it('checks again after a failure and confirms on success', async () => {
    me.mockRejectedValueOnce(new TypeError('Failed to fetch')).mockResolvedValueOnce(user)

    await session.revalidate()
    expect(session.status).toBe('unreachable')

    await session.retry()

    expect(me).toHaveBeenCalledTimes(2)
    expect(session.status).toBe('confirmed')
    expect(session.user).toEqual(user)
  })
})

describe('session.set / session.patch', () => {
  it('set replaces the user and writes through', () => {
    session.set(user)

    expect(session.user).toEqual(user)
    expect(getSessionUser()).toEqual(user)
  })

  it('patch updates fields of the known user and writes through', () => {
    session.set({ ...user, has_password: false })

    session.patch({ has_password: true, oauth_providers: ['google'] })

    const expected = { ...user, has_password: true, oauth_providers: ['google'] }
    expect(session.user).toEqual(expected)
    expect(getSessionUser()).toEqual(expected)
  })

  it('patch does nothing while the user is unknown', () => {
    session.patch({ has_password: true })

    expect(session.user).toBeNull()
    expect(getSessionUser()).toBeNull()
  })
})

describe('session.signOut', () => {
  beforeEach(() => {
    session.set(user)
  })

  it('clears the user, cache and session hint before asking the server', async () => {
    let seenAtRequest: unknown[] = []
    logout.mockImplementation(() => {
      seenAtRequest = [session.user, getSessionUser(), getTokenExpiry()]
      return Promise.resolve()
    })

    await session.signOut()

    expect(logout).toHaveBeenCalledWith({ keepalive: true })
    expect(seenAtRequest).toEqual([null, null, null])
  })

  it('navigates to /login as soon as logout resolves', async () => {
    vi.useFakeTimers()
    const answer = deferred<void>()
    logout.mockReturnValue(answer.promise)

    const done = session.signOut()
    await vi.advanceTimersByTimeAsync(100)
    expect(navigateReplace).not.toHaveBeenCalled()

    answer.resolve()
    await done

    expect(navigateReplace).toHaveBeenCalledWith('/login')
  })

  it('navigates to /login after 1 s when logout hangs', async () => {
    vi.useFakeTimers()
    logout.mockReturnValue(new Promise(() => {}))

    const done = session.signOut()
    await vi.advanceTimersByTimeAsync(SIGN_OUT_TIMEOUT_MS - 1)
    expect(navigateReplace).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(1)
    await done

    expect(SIGN_OUT_TIMEOUT_MS).toBe(1000)
    expect(navigateReplace).toHaveBeenCalledWith('/login')
  })

  it('leaves no session hint when a refresh during logout rewrote it', async () => {
    vi.useFakeTimers()
    // A logout without a CSRF cookie refreshes first, which re-sets the hint,
    // and then hangs, so the timeout wins.
    logout.mockImplementation(() => {
      setTokenExpiry(1_800_000_000)
      return new Promise(() => {})
    })

    const done = session.signOut()
    await vi.advanceTimersByTimeAsync(SIGN_OUT_TIMEOUT_MS)
    await done

    expect(getTokenExpiry()).toBeNull()
    expect(getSessionUser()).toBeNull()
    expect(navigateReplace).toHaveBeenCalledWith('/login')
  })

  it('navigates to /login when logout rejects, swallowing the error', async () => {
    vi.useFakeTimers()
    logout.mockRejectedValue(new TypeError('Failed to fetch'))

    await expect(session.signOut()).resolves.toBeUndefined()

    expect(navigateReplace).toHaveBeenCalledWith('/login')
  })

  it('ignores a late /auth/me answer after signing out', async () => {
    const answer = deferred<User>()
    me.mockReturnValue(answer.promise)
    logout.mockResolvedValue(undefined)

    const check = session.revalidate()
    await session.signOut()
    answer.resolve(user)
    await check

    expect(session.user).toBeNull()
    expect(getSessionUser()).toBeNull()
  })
})
