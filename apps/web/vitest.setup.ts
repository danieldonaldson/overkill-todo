import { beforeEach, vi } from 'vitest'

beforeEach(() => {
  vi.clearAllMocks()
  // Storage is per-origin, so it outlives a test file: the session store
  // caches the signed-in user in sessionStorage and the session hint in
  // localStorage. Without this, which user a test sees depends on file order.
  sessionStorage.clear()
  localStorage.clear()
})
