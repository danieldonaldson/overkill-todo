import { afterEach, describe, expect, it, vi } from 'vitest'

import { goBack, navigate } from './navigation'

// navigateReplace() and redirect() are intentionally not unit-tested: they
// assign to window.location, which cannot be safely intercepted in the real
// browser test environment. They are exercised end-to-end in the Playwright
// suite (session-expiry redirect to /login).
describe('navigation helpers', () => {
  const originalUrl = window.location.href

  afterEach(() => {
    vi.restoreAllMocks()
    // Restore the URL after any pushState-based navigation.
    window.history.replaceState({}, '', originalUrl)
  })

  it('navigate() performs a soft (history) navigation via Astro', () => {
    navigate('/dashboard/settings')
    expect(window.location.pathname).toBe('/dashboard/settings')
  })

  it('goBack() steps back through history', () => {
    const back = vi.spyOn(window.history, 'back').mockImplementation(() => {})
    goBack()
    expect(back).toHaveBeenCalledTimes(1)
  })
})
