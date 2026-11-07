/**
 * Per-browser device binding for refresh token rotation.
 *
 * This is intentionally an opaque random ID, not a hardware/browser fingerprint.
 * The backend only needs a stable value so a stolen refresh token cannot be
 * rotated from a different browser without also having this local value.
 */

const FINGERPRINT_STORAGE_KEY = 'device_fingerprint'
const FINGERPRINT_BYTES = 32

function randomHex(bytes: number): string {
  const values = new Uint8Array(bytes)

  if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
    crypto.getRandomValues(values)
  } else {
    for (let i = 0; i < values.length; i += 1) {
      values[i] = Math.floor(Math.random() * 256)
    }
  }

  return Array.from(values, (value) => value.toString(16).padStart(2, '0')).join('')
}

/**
 * Gets the per-browser device binding, generating and storing it if necessary.
 */
export async function getDeviceFingerprint(): Promise<string> {
  if (typeof window === 'undefined' || typeof localStorage === 'undefined') {
    return randomHex(FINGERPRINT_BYTES)
  }

  const cached = localStorage.getItem(FINGERPRINT_STORAGE_KEY)
  if (cached) {
    return cached
  }

  const fingerprint = randomHex(FINGERPRINT_BYTES)
  try {
    localStorage.setItem(FINGERPRINT_STORAGE_KEY, fingerprint)
  } catch {
    // localStorage might be full or disabled.
  }

  return fingerprint
}

/**
 * Clears the stored device binding.
 */
export function clearDeviceFingerprint(): void {
  if (typeof localStorage !== 'undefined') {
    localStorage.removeItem(FINGERPRINT_STORAGE_KEY)
  }
}

/**
 * Regenerates the device binding by clearing the cache and generating a new one.
 */
export async function regenerateFingerprint(): Promise<string> {
  clearDeviceFingerprint()
  return getDeviceFingerprint()
}
