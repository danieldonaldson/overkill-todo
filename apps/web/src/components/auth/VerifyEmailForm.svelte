<script lang="ts">
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert'
  import CircleCheckIcon from '@lucide/svelte/icons/circle-check'
  import { Alert, AlertDescription } from '$lib/components/ui/alert'
  import { Button } from '$lib/components/ui/button'
  import CodeInput from '../ui/custom/CodeInput.svelte'
  import CountdownTimer from '../ui/custom/CountdownTimer.svelte'
  import Turnstile, { type TurnstileRef } from '../ui/custom/Turnstile.svelte'
  import { api } from '../../lib/api'
  import { navigate } from '../../lib/navigation'

  let { email: propEmail = '' }: { email?: string } = $props()

  const email = $derived(propEmail || getEmailFromUrl())
  const resendDisabled = $derived(resending || cooldownSeconds > 0)

  let code = $state('')
  let turnstileToken = $state<string | null>(null)
  let codeError = $state<string | null>(null)
  let formError = $state<string | null>(null)
  let successMessage = $state<string | null>(null)
  let loading = $state(false)
  let resending = $state(false)
  let cooldownSeconds = $state(0)
  let turnstileRef = $state<TurnstileRef | undefined>(undefined)

  function getEmailFromUrl(): string {
    if (typeof window === 'undefined') return ''
    return new URLSearchParams(window.location.search).get('email') || ''
  }

  async function handleVerify(submittedCode?: string) {
    const verifyCode = submittedCode || code
    if (verifyCode.length !== 6) return

    loading = true
    formError = null
    successMessage = null
    codeError = null

    try {
      const response = await api.auth.verifyEmail({ email, code: verifyCode })

      if (response.signup_token) {
        const params = new URLSearchParams({ token: response.signup_token, email })
        navigate(`/set-password?${params.toString()}`)
      } else if (response.user) {
        // Existing user with a password: auto-logged-in via HttpOnly cookies.
        navigate('/dashboard')
      } else {
        formError = 'Verification succeeded but no redirect path. Please try logging in.'
      }
    } catch (error: unknown) {
      if (error && typeof error === 'object' && 'error_code' in error) {
        const authError = error as {
          error?: string
          error_code: string
          attempts_remaining?: number
        }

        if (authError.error_code === 'CODE_INVALID') {
          codeError =
            authError.attempts_remaining !== undefined
              ? `Incorrect code. ${authError.attempts_remaining} attempts remaining.`
              : 'Incorrect code.'
        } else if (authError.error_code === 'CODE_EXPIRED') {
          formError = 'This code has expired. Request a new one below.'
        } else if (authError.error_code === 'CODE_LOCKED') {
          formError = 'Too many incorrect attempts. Request a new code below.'
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

  async function handleSubmit(event: SubmitEvent) {
    event.preventDefault()
    await handleVerify()
  }

  function handleCodeComplete(completedCode: string) {
    void handleVerify(completedCode)
  }

  async function handleResend() {
    if (!email) {
      formError = 'Email address is required to resend code'
      return
    }

    resending = true
    formError = null
    successMessage = null
    codeError = null

    try {
      const response = await api.auth.resendVerification({
        email,
        turnstile_token: turnstileToken || undefined,
      })
      successMessage = 'A new verification code has been sent. Check your email.'
      cooldownSeconds = response.cooldown_seconds ?? 0
      turnstileRef?.reset()
      turnstileToken = null
      code = ''
    } catch (error: unknown) {
      if (error && typeof error === 'object' && 'error_code' in error) {
        const authError = error as { error?: string; error_code: string; cooldown_seconds?: number }
        if (authError.error_code === 'COOLDOWN_ACTIVE' && authError.cooldown_seconds) {
          cooldownSeconds = authError.cooldown_seconds
          formError = 'Please wait before requesting a new code.'
        } else if (authError.error_code === 'ALREADY_VERIFIED') {
          navigate('/dashboard')
        } else {
          formError = authError.error || 'An unexpected error occurred'
        }
      } else {
        formError = error instanceof Error ? error.message : 'An unexpected error occurred'
      }
      turnstileRef?.reset()
      turnstileToken = null
    } finally {
      resending = false
    }
  }

  function handleCooldownComplete() {
    cooldownSeconds = 0
  }

  function handleTurnstileVerify(token: string) {
    turnstileToken = token || null
  }
</script>

<form class="flex flex-col gap-4" method="post" onsubmit={handleSubmit}>
  {#if formError}
    <Alert variant="destructive">
      <CircleAlertIcon class="size-4" />
      <AlertDescription>{formError}</AlertDescription>
    </Alert>
  {/if}

  {#if successMessage}
    <Alert>
      <CircleCheckIcon class="size-4 text-primary" />
      <AlertDescription>{successMessage}</AlertDescription>
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
    {loading ? 'Verifying...' : 'Verify email'}
  </Button>

  <div class="my-2 h-px bg-border"></div>

  <div class="flex flex-col gap-3">
    {#if cooldownSeconds > 0}
      <CountdownTimer seconds={cooldownSeconds} oncomplete={handleCooldownComplete} />
    {:else}
      <Turnstile bind:this={turnstileRef} onVerify={handleTurnstileVerify} />
      <Button type="button" variant="ghost" disabled={resendDisabled} onclick={handleResend} class="w-full">
        {#if resending}
          <span class="size-4 animate-spin rounded-full border-2 border-current border-r-transparent"></span>
        {/if}
        Resend code
      </Button>
    {/if}
  </div>
</form>
