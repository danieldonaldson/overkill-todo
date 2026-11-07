import type {
  LoginRequest,
  RegisterRequest,
  InitiateRegistrationResponse,
  CompleteSignupRequest,
  AuthResponse,
  RefreshResponse,
  User,
  ForgotPasswordRequest,
  ResetPasswordRequest,
  VerifyEmailRequest,
  VerifyEmailResponse,
  ResendVerificationRequest,
  ResendVerificationResponse,
  VerifyResetCodeRequest,
  VerifyResetCodeResponse,
} from '../types'
import { HttpClient } from './client'
import { setTokenExpiry, clearTokenExpiry } from '../auth/token'

export function createAuthApi(client: HttpClient) {
  return {
    login: async (data: LoginRequest): Promise<AuthResponse> => {
      const response = await client.requestWithFingerprint<AuthResponse>('/auth/login', {
        method: 'POST',
        body: JSON.stringify(data),
      })
      setTokenExpiry(response.expires_at)
      return response
    },

    // Step 1: Initiate registration (email only, no password)
    register: async (data: RegisterRequest): Promise<InitiateRegistrationResponse> => {
      return client.request<InitiateRegistrationResponse>('/auth/register', {
        method: 'POST',
        body: JSON.stringify(data),
      })
    },

    // Step 3: Complete signup by setting password (after email verification)
    completeSignup: async (data: CompleteSignupRequest): Promise<AuthResponse> => {
      const response = await client.requestWithFingerprint<AuthResponse>('/auth/complete-signup', {
        method: 'POST',
        body: JSON.stringify(data),
      })
      setTokenExpiry(response.expires_at)
      return response
    },

    me: (): Promise<User> => client.request<User>('/auth/me'),

    logout: async (): Promise<void> => {
      await client.request<void>('/auth/logout', {
        method: 'POST',
      })
      clearTokenExpiry()
    },

    refresh: async (): Promise<RefreshResponse> => {
      const data = await client.requestWithFingerprint<RefreshResponse>('/auth/refresh', {
        method: 'POST',
      })
      setTokenExpiry(data.expires_at)
      return data
    },

    forgotPassword: (data: ForgotPasswordRequest): Promise<void> =>
      client.request('/auth/forgot-password', {
        method: 'POST',
        body: JSON.stringify(data),
      }),

    verifyResetCode: (data: VerifyResetCodeRequest): Promise<VerifyResetCodeResponse> =>
      client.request('/auth/verify-reset-code', {
        method: 'POST',
        body: JSON.stringify(data),
      }),

    resetPassword: async (data: ResetPasswordRequest): Promise<AuthResponse> => {
      const response = await client.requestWithFingerprint<AuthResponse>('/auth/reset-password', {
        method: 'POST',
        body: JSON.stringify(data),
      })
      setTokenExpiry(response.expires_at)
      return response
    },

    // Step 2: Verify email with 6-digit code.
    // New users (no password) get a `signup_token`; existing users with a
    // password are auto-logged-in via HttpOnly cookies (no body tokens). The
    // auto-login response carries `expires_at` so we record it here and the
    // client can refresh proactively rather than waiting for the first 401.
    verifyEmail: async (data: VerifyEmailRequest): Promise<VerifyEmailResponse> => {
      const response = await client.requestWithFingerprint<VerifyEmailResponse>(
        '/auth/verify-email',
        {
          method: 'POST',
          body: JSON.stringify(data),
        },
      )
      if (response.expires_at !== undefined) {
        setTokenExpiry(response.expires_at)
      }
      return response
    },

    resendVerification: (data: ResendVerificationRequest): Promise<ResendVerificationResponse> =>
      client.request('/auth/resend-verification', {
        method: 'POST',
        body: JSON.stringify(data),
      }),
  }
}
