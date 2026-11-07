import { describe, expect, it } from 'vitest'

import { api } from './index'

// The exported `api` is a Proxy over a lazily-constructed ApiClient. These
// tests assert the endpoint groups are wired up and that the singleton is
// reused across property accesses.
describe('api client singleton', () => {
  it('exposes every endpoint group', () => {
    expect(api.auth).toBeTruthy()
    expect(api.oauth).toBeTruthy()
    expect(api.users).toBeTruthy()
    expect(api.admin).toBeTruthy()
  })

  it('returns the same lazily-initialized instance across accesses', () => {
    expect(api.auth).toBe(api.auth)
    expect(api.users).toBe(api.users)
  })
})
