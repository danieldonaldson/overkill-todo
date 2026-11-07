import { beforeEach, describe, expect, it } from 'vitest'
import {
  clearDeviceFingerprint,
  getDeviceFingerprint,
  regenerateFingerprint,
} from './fingerprint'

const STORAGE_KEY = 'device_fingerprint'
const SHA256_HEX = /^[0-9a-f]{64}$/

beforeEach(() => {
  localStorage.clear()
})

describe('getDeviceFingerprint', () => {
  it('generates a SHA-256 hex fingerprint', async () => {
    const fingerprint = await getDeviceFingerprint()
    expect(fingerprint).toMatch(SHA256_HEX)
  })

  it('persists the generated fingerprint to localStorage', async () => {
    const fingerprint = await getDeviceFingerprint()
    expect(localStorage.getItem(STORAGE_KEY)).toBe(fingerprint)
  })

  it('returns the same fingerprint across calls on the same device', async () => {
    const first = await getDeviceFingerprint()
    const second = await getDeviceFingerprint()
    expect(second).toBe(first)
  })

  it('returns a cached value verbatim without regenerating', async () => {
    localStorage.setItem(STORAGE_KEY, 'cached-fingerprint-value')
    expect(await getDeviceFingerprint()).toBe('cached-fingerprint-value')
  })
})

describe('clearDeviceFingerprint', () => {
  it('removes the stored fingerprint', async () => {
    await getDeviceFingerprint()
    expect(localStorage.getItem(STORAGE_KEY)).not.toBeNull()

    clearDeviceFingerprint()

    expect(localStorage.getItem(STORAGE_KEY)).toBeNull()
  })
})

describe('regenerateFingerprint', () => {
  it('replaces a cached value with a freshly generated fingerprint', async () => {
    localStorage.setItem(STORAGE_KEY, 'stale-value')

    const fingerprint = await regenerateFingerprint()

    expect(fingerprint).toMatch(SHA256_HEX)
    expect(fingerprint).not.toBe('stale-value')
    expect(localStorage.getItem(STORAGE_KEY)).toBe(fingerprint)
  })
})
