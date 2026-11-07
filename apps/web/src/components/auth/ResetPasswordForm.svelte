<script lang="ts">
  import CheckIcon from '@lucide/svelte/icons/check'
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert'
  import { Alert, AlertDescription } from '$lib/components/ui/alert'
  import { Button } from '$lib/components/ui/button'
  import { FormInput } from '$lib/components/ui/form-input'
  import CodeInput from '../ui/custom/CodeInput.svelte'
  import PasswordStrength from '../ui/custom/PasswordStrength.svelte'
  import { api } from '../../lib/api'
  import { navigate } from '../../lib/navigation'
  import { minLength, required, validate } from '../../lib/utils/validation'

  let { email: propEmail = '' }: { email?: string } = $props()

  const email = $derived(propEmail || getEmailFromUrl())
  const passwordsMatch = $derived(confirmPassword.length > 0 && confirmPassword === password)

  let step = $state<'code' | 'password'>('code')
  let resetToken = $state<string | null>(null)
  let code = $state('')
  let password = $state('')
  let confirmPassword = $state('')
  let codeError = $state<string | null>(null)
  let passwordError = $state<string | null>(null)
  let confirmPasswordError = $state<string | null>(null)
  let formError = $state<string | null>(null)
  let loading = $state(false)

  function getEmailFromUrl(): string {
    if (typeof window === 'undefined') return ''
    return new URLSearchParams(window.location.search).get('email') || ''
  }

  function validatePassword() {
    passwordError = validate(password, [
      required(),
      minLength(8, 'Password must be at least 8 characters'),
    ])
  }

  function validateConfirmPassword() {
    if (!confirmPassword) {
      confirmPasswordError = 'Please confirm your password'
    } else if (confirmPassword !== password) {
      confirmPasswordError = 'Passwords do not match'
    } else {
      confirmPasswordError = null
    }
  }

  async function handleVerifyCode(submittedCode?: string) {
    const verifyCode = submittedCode || code
    if (verifyCode.length !== 6) return

    loading = true
    formError = null
    codeError = null

    try {
      const response = await api.auth.verifyResetCode({ email, code: verifyCode })
      resetToken = response.reset_token
      step = 'password'
    } catch (error: unknown) {
      if (error && typeof error === 'object' && 'error_code' in error) {
        const authError = error as { error?: string; error_code: string; attempts_remaining?: number }
        if (authError.error_code === 'CODE_INVALID') {
          codeError =
            authError.attempts_remaining !== undefined
              ? `Incorrect code. ${authError.attempts_remaining} attempts remaining.`
              : 'Incorrect code.'
        } else if (authError.error_code === 'CODE_EXPIRED') {
          formError = 'This code has expired. Please request a new one.'
        } else if (authError.error_code === 'CODE_LOCKED') {
          formError = 'Too many incorrect attempts. Please request a new code.'
        } else {
          formError = authError.error || 'An unexpected error occurred'
        }
      } else {
        formError = error instanceof Error ? error.message : 'An unexpected error occurred'
      }
      code = ''
    } finally {
      loading = false
    }
  }

  async function handleResetPassword(event: SubmitEvent) {
    event.preventDefault()
    validatePassword()
    validateConfirmPassword()
    if (passwordError || confirmPasswordError || !resetToken) return

    loading = true
    formError = null

    try {
      await api.auth.resetPassword({
        reset_token: resetToken,
        new_password: password,
      })
      navigate('/dashboard')
    } catch (error: unknown) {
      if (error && typeof error === 'object' && 'error_code' in error) {
        const authError = error as { error?: string; error_code: string }
        if (authError.error_code === 'TOKEN_EXPIRED') {
          formError = 'Your session expired. Please enter your code again.'
          step = 'code'
          resetToken = null
        } else {
          formError = authError.error || 'An unexpected error occurred'
        }
      } else {
        formError = error instanceof Error ? error.message : 'An unexpected error occurred'
      }
    } finally {
      loading = false
    }
  }

  function handleCodeComplete(completedCode: string) {
    void handleVerifyCode(completedCode)
  }

  function handleCodeSubmit(event: SubmitEvent) {
    event.preventDefault()
    void handleVerifyCode()
  }
</script>

{#if step === 'code'}
  <form class="flex flex-col gap-4" method="post" onsubmit={handleCodeSubmit}>
    {#if formError}
      <Alert variant="destructive">
        <CircleAlertIcon class="size-4" />
        <AlertDescription>{formError}</AlertDescription>
      </Alert>
    {/if}

    <p class="text-center text-sm text-muted-foreground">
      Enter the 6-digit code sent to <strong class="text-foreground">{email}</strong>
    </p>

    <CodeInput bind:value={code} disabled={loading} error={codeError} oncomplete={handleCodeComplete} />

    <Button type="submit" disabled={loading || code.length !== 6} class="w-full">
      {#if loading}
        <span class="size-4 animate-spin rounded-full border-2 border-current border-r-transparent"></span>
      {/if}
      {loading ? 'Verifying...' : 'Verify code'}
    </Button>

    <p class="text-center text-sm text-muted-foreground">
      <a href="/login" class="font-medium text-primary hover:underline">Back to sign in</a>
    </p>
  </form>
{:else}
  <form class="flex flex-col gap-4" method="post" onsubmit={handleResetPassword}>
    {#if formError}
      <Alert variant="destructive">
        <CircleAlertIcon class="size-4" />
        <AlertDescription>{formError}</AlertDescription>
      </Alert>
    {/if}

    <p class="text-center text-sm text-muted-foreground">Code verified! Enter your new password.</p>

    <div class="flex flex-col gap-1">
      <FormInput
        type="password"
        name="password"
        label="New password"
        placeholder="At least 8 characters"
        bind:value={password}
        error={passwordError}
        required
        autocomplete="new-password"
        showPasswordToggle
        onblur={validatePassword}
      />
      <PasswordStrength {password} />
    </div>

    <div class="flex flex-col gap-1">
      <FormInput
        type="password"
        name="confirmPassword"
        label="Confirm new password"
        placeholder="Confirm your new password"
        bind:value={confirmPassword}
        error={confirmPasswordError}
        required
        autocomplete="new-password"
        showPasswordToggle
        onblur={validateConfirmPassword}
      />
      {#if passwordsMatch}
        <div class="flex items-center gap-2 text-sm text-primary">
          <CheckIcon class="size-4" />
          <span>Passwords match</span>
        </div>
      {/if}
    </div>

    <Button type="submit" disabled={loading} class="w-full">
      {#if loading}
        <span class="size-4 animate-spin rounded-full border-2 border-current border-r-transparent"></span>
      {/if}
      {loading ? 'Resetting...' : 'Reset password'}
    </Button>
  </form>
{/if}
