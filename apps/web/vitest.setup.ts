import { beforeEach, vi } from 'vitest'

beforeEach(() => {
  vi.clearAllMocks()
  // Storage is per-origin, so it outlives a test file: components such as
  // AdminPanel cache the signed-in user in sessionStorage, which AuthProvider
  // then reads back. Without this, which user a test sees depends on file order.
  sessionStorage.clear()
  localStorage.clear()
})
