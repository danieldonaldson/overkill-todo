import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  clearTokenExpiry,
  ensureValidToken,
  getTimeUntilExpiry,
  getTokenExpiry,
  isTokenExpired,
  isTokenExpiringSoon,
  resetTokenManagerState,
  setTokenExpiry,
} from './token'

const now = () => Math.floor(Date.now() / 1000)

beforeEach(() => {
  localStorage.clear()
  resetTokenManagerState()
})

describe('token expiry storage', () => {
  it('round-trips a stored expiry timestamp', () => {
    setTokenExpiry(1234567890)
    expect(getTokenExpiry()).toBe(1234567890)
  })

  it('returns null when no expiry is stored', () => {
    expect(getTokenExpiry()).toBeNull()
  })

  it('returns null when the stored value is not a number', () => {
    localStorage.setItem('jwt_expiry', 'not-a-number')
    expect(getTokenExpiry()).toBeNull()
  })

  it('clears the stored expiry', () => {
    setTokenExpiry(now() + 100)
    clearTokenExpiry()
    expect(getTokenExpiry()).toBeNull()
  })
})

describe('isTokenExpired', () => {
  it('treats a missing expiry as expired', () => {
    expect(isTokenExpired()).toBe(true)
  })

  it('returns true for a past expiry', () => {
    setTokenExpiry(now() - 10)
    expect(isTokenExpired()).toBe(true)
  })

  it('returns false for a future expiry', () => {
    setTokenExpiry(now() + 3600)
    expect(isTokenExpired()).toBe(false)
  })
})

describe('isTokenExpiringSoon', () => {
  it('returns false when no expiry is stored', () => {
    expect(isTokenExpiringSoon()).toBe(false)
  })

  it('returns true when within the 60s refresh threshold', () => {
    setTokenExpiry(now() + 30)
    expect(isTokenExpiringSoon()).toBe(true)
  })

  it('returns false when comfortably in the future', () => {
    setTokenExpiry(now() + 3600)
    expect(isTokenExpiringSoon()).toBe(false)
  })

  it('returns true once the token is already expired', () => {
    setTokenExpiry(now() - 10)
    expect(isTokenExpiringSoon()).toBe(true)
  })
})

describe('getTimeUntilExpiry', () => {
  it('returns 0 when no expiry is stored', () => {
    expect(getTimeUntilExpiry()).toBe(0)
  })

  it('returns 0 when already expired', () => {
    setTokenExpiry(now() - 100)
    expect(getTimeUntilExpiry()).toBe(0)
  })

  it('returns the remaining seconds for a future expiry', () => {
    setTokenExpiry(now() + 120)
    // Allow a small window for clock movement during the test.
    expect(getTimeUntilExpiry()).toBeGreaterThan(115)
    expect(getTimeUntilExpiry()).toBeLessThanOrEqual(120)
  })
})

describe('ensureValidToken', () => {
  it('does not refresh when the token is not expiring soon', async () => {
    setTokenExpiry(now() + 3600)
    const refresh = vi.fn().mockResolvedValue(true)

    const result = await ensureValidToken(refresh)

    expect(result).toBe(true)
    expect(refresh).not.toHaveBeenCalled()
  })

  it('refreshes when the token is expiring soon', async () => {
    setTokenExpiry(now() + 30)
    const refresh = vi.fn().mockResolvedValue(true)

    const result = await ensureValidToken(refresh)

    expect(result).toBe(true)
    expect(refresh).toHaveBeenCalledTimes(1)
  })

  it('propagates a failed refresh result', async () => {
    setTokenExpiry(now() + 30)
    const refresh = vi.fn().mockResolvedValue(false)

    expect(await ensureValidToken(refresh)).toBe(false)
  })

  it('dedupes concurrent refreshes into a single call', async () => {
    setTokenExpiry(now() + 30)
    let resolveRefresh: (value: boolean) => void = () => {}
    const refresh = vi.fn(
      () =>
        new Promise<boolean>((resolve) => {
          resolveRefresh = resolve
        })
    )

    const first = ensureValidToken(refresh)
    const second = ensureValidToken(refresh)
    // The cross-tab Web Lock invokes refresh asynchronously, so wait until it
    // has actually started before resolving it.
    await vi.waitFor(() => expect(refresh).toHaveBeenCalledTimes(1))
    resolveRefresh(true)

    expect(await first).toBe(true)
    expect(await second).toBe(true)
    expect(refresh).toHaveBeenCalledTimes(1)
  })

  it('allows a new refresh after a previous one settles', async () => {
    setTokenExpiry(now() + 30)
    const refresh = vi.fn().mockResolvedValue(true)

    await ensureValidToken(refresh)
    await ensureValidToken(refresh)

    expect(refresh).toHaveBeenCalledTimes(2)
  })

  it('releases the lock when a wedged refresh is reset, unblocking the next caller', async () => {
    setTokenExpiry(now() + 30)
    // A refresh that never settles would otherwise hold the Web Lock forever.
    const wedged = vi.fn(() => new Promise<boolean>(() => {}))
    const stuck = ensureValidToken(wedged)
    await vi.waitFor(() => expect(wedged).toHaveBeenCalledTimes(1))

    // Reset aborts the in-flight refresh and releases the lock.
    resetTokenManagerState()
    await expect(stuck).resolves.toBe(false)

    // A fresh caller can now acquire the lock and complete.
    setTokenExpiry(now() + 30)
    const next = vi.fn().mockResolvedValue(true)
    await expect(ensureValidToken(next)).resolves.toBe(true)
    expect(next).toHaveBeenCalledTimes(1)
  })
})
