// API payload types for the app, under the names the frontend uses.
//
// Every payload shape here is an alias of a type generated from the backend's
// OpenAPI description (`./api/schema.d.ts`, from `backend/openapi.json`). Don't
// declare payload fields here: change the backend DTO, then run `just api-types`.

import type { components } from './api/schema'

type Schemas = components['schemas']

export type UserRole = Schemas['RoleName']

/**
 * OAuth providers this UI offers a button for. Narrower than the API's set
 * (`Schemas['OAuthProviderName']`), which also includes providers the backend
 * supports but the UI doesn't show.
 */
export type OAuthProvider = Extract<Schemas['OAuthProviderName'], 'google'>

/** Whether a role grants admin-panel access (admin or super-admin). */
export function isAdminRole(role: UserRole | undefined | null): boolean {
  return role === 'admin' || role === 'super_admin'
}

export type User = Schemas['UserResponse']

// --- Auth requests/responses ---

export type LoginRequest = Schemas['LoginRequest']
/** Email-first signup, step 1: initiate registration (email only). */
export type RegisterRequest = Schemas['InitiateRegistrationRequest']
export type InitiateRegistrationResponse = Schemas['InitiateRegistrationResponse']
/** Email-first signup, step 3: complete signup by setting a password. */
export type CompleteSignupRequest = Schemas['CompleteRegistrationRequest']
export type ForgotPasswordRequest = Schemas['ForgotPasswordRequest']
export type ForgotPasswordResponse = Schemas['ForgotPasswordResponse']
export type VerifyResetCodeRequest = Schemas['VerifyResetCodeRequest']
export type VerifyResetCodeResponse = Schemas['VerifyResetCodeResponse']
export type ResetPasswordRequest = Schemas['ResetPasswordRequest']
export type VerifyEmailRequest = Schemas['VerifyEmailRequest']
/**
 * Response after email verification.
 * New users (no password) get a `signup_token`; existing users with a password
 * are auto-logged-in via HttpOnly cookies (no tokens are returned in the body).
 */
export type VerifyEmailResponse = Schemas['VerifyEmailResponse']
export type ResendVerificationRequest = Schemas['ResendVerificationRequest']
export type ResendVerificationResponse = Schemas['ResendVerificationResponse']
export type AuthResponse = Schemas['AuthResponse']
export type LogoutResponse = Schemas['LogoutResponse']

// Login and refresh return the identical body shape (`AuthResponse`).
export type RefreshResponse = AuthResponse

// --- Profile (password) ---

export type ChangePasswordRequest = Schemas['ChangePasswordRequest']
export type SetPasswordRequest = Schemas['SetPasswordRequest']

// --- Account data export / deletion ---

export type RequestDeletionRequest = Schemas['RequestDeletionRequest']
export type LinkedOAuthAccountExport = Schemas['LinkedOAuthAccountExport']
export type AccountExportResponse = Schemas['AccountExportResponse']
export type AccountDeletionResponse = Schemas['AccountDeletionResponse']

// --- Admin ---

/** A user row in the admin user-management table. */
export type AdminUserRow = Schemas['AdminUserResponse']
export type AdminUserList = Schemas['AdminUserListResponse']
