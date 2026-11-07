// Auth + user types for the app dashboard.
// Shapes mirror the backend's UserResponse / auth DTOs (`/api/v1/...`).

export type UserRole = 'user' | 'admin' | 'super_admin'
export type OAuthProvider = 'google'

/** Whether a role grants admin-panel access (admin or super-admin). */
export function isAdminRole(role: UserRole | undefined | null): boolean {
  return role === 'admin' || role === 'super_admin'
}

export interface User {
  id: string
  email: string
  first_name: string
  last_name: string
  email_verified: boolean
  has_password: boolean
  /** Provider names the user has linked, e.g. ["google"]. */
  oauth_providers: string[]
  pending_email?: string | null
  pending_email_requested_at?: string | null
  deletion_scheduled_at?: string | null
  role?: UserRole
  created_at?: string
}

// --- Auth requests/responses ---

export interface LoginRequest {
  email: string
  password: string
  turnstile_token?: string
}

/** Email-first signup, step 1: initiate registration (email only). */
export interface RegisterRequest {
  email: string
  turnstile_token?: string
}

export interface InitiateRegistrationResponse {
  success: boolean
  message: string
}

/** Email-first signup, step 3: complete signup by setting a password. */
export interface CompleteSignupRequest {
  signup_token: string
  email: string
  password: string
}

export interface ForgotPasswordRequest {
  email: string
  turnstile_token?: string
}

export interface VerifyResetCodeRequest {
  email: string
  code: string
}

export interface VerifyResetCodeResponse {
  reset_token: string
}

export interface ResetPasswordRequest {
  reset_token: string
  new_password: string
}

export interface VerifyEmailRequest {
  email: string
  code: string
}

/**
 * Response after email verification.
 * New users (no password) get a `signup_token`; existing users with a password
 * are auto-logged-in via HttpOnly cookies (no tokens are returned in the body).
 */
export interface VerifyEmailResponse {
  signup_token?: string
  user?: User
  /** Unix-seconds access-token expiry; present only on the auto-login path. */
  expires_at?: number
}

export interface ResendVerificationRequest {
  email: string
  turnstile_token?: string
}

export interface ResendVerificationResponse {
  success?: boolean
  message?: string
  cooldown_seconds?: number
}

export interface AuthResponse {
  expires_at: number
  csrf_token?: string
  user: User
}

// Login and refresh return the identical body shape; the backend collapsed them
// onto one `AuthResponse` schema, so the refresh alias points at it.
export type RefreshResponse = AuthResponse

// --- Profile (password) ---

export interface ChangePasswordRequest {
  current_password: string
  new_password: string
}

export interface SetPasswordRequest {
  new_password: string
}

// --- Account data export / deletion ---

export interface RequestDeletionRequest {
  /** Current password — required for password accounts, omitted for OAuth-only. */
  password?: string
  /** Must be the literal "DELETE" to confirm. */
  confirmation: string
}

export interface LinkedOAuthAccountExport {
  provider: string
  provider_email?: string
  linked_at: string
}

export interface AccountExportResponse {
  id: string
  email: string
  first_name: string
  last_name: string
  email_verified: boolean
  has_password: boolean
  pending_email?: string
  deletion_scheduled_at?: string
  role: string
  created_at: string
  updated_at: string
  linked_oauth_accounts: LinkedOAuthAccountExport[]
  exported_at: string
}

export interface AccountDeletionResponse {
  deletion_scheduled_at?: string
}

// --- Admin ---

/** A user row in the admin user-management table. */
export interface AdminUserRow {
  id: string
  email: string
  role: UserRole
  email_verified: boolean
  created_at: string
  deletion_scheduled_at?: string | null
}

export interface AdminUserList {
  users: AdminUserRow[]
  limit: number
  offset: number
}
