import type { User } from '../types'

/**
 * Auth context key for Svelte's setContext/getContext.
 */
export const AUTH_CONTEXT_KEY = Symbol('auth')

/**
 * Auth context type provided by AuthProvider.
 */
export interface AuthContext {
  readonly user: User | null
  readonly isAuthenticated: boolean
  readonly isLoading: boolean
  logout: () => Promise<void>
}
