<script lang="ts">
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert'
  import { Alert, AlertDescription } from '$lib/components/ui/alert'
  import { Button } from '$lib/components/ui/button'
  import { FormInput } from '$lib/components/ui/form-input'
  import { api } from '../../lib/api'
  import { navigate } from '../../lib/navigation'
  import { email, minLength, required, validate } from '../../lib/utils/validation'
  import OAuthButtons from './OAuthButtons.svelte'

  // The caller owns the Turnstile widget so it outlives a tab switch: remounting
  // it would drop the issued token and burn a fresh challenge on every toggle.
  interface Props {
    emailValue?: string
    turnstileToken?: string | null
    resetTurnstile?: () => void
  }

  let { emailValue = $bindable(''), turnstileToken = null, resetTurnstile = () => {} }: Props = $props()

  let password = $state('')
  let emailError = $state<string | null>(null)
  let passwordError = $state<string | null>(null)
  let formError = $state<string | null>(null)
  let loading = $state(false)

  function validateEmail() {
    emailError = validate(emailValue, [required(), email()])
  }

  function validatePassword() {
    passwordError = validate(password, [required(), minLength(8)])
  }

  async function handleSubmit(event: SubmitEvent) {
    event.preventDefault()

    validateEmail()
    validatePassword()

    if (emailError || passwordError) return

    loading = true
    formError = null

    try {
      await api.auth.login({
        email: emailValue,
        password,
        turnstile_token: turnstileToken || undefined,
      })
      navigate('/dashboard')
    } catch (error: unknown) {
      if (error && typeof error === 'object' && 'error_code' in error) {
        const authError = error as { error?: string; message?: string; error_code: string; email?: string }

        if (authError.error_code === 'signup_incomplete') {
          navigate(`/verify-email?email=${encodeURIComponent(authError.email || emailValue)}`)
          return
        }

        formError = authError.error || authError.message || 'An unexpected error occurred'
      } else {
        formError = error instanceof Error ? error.message : 'An unexpected error occurred'
      }

      resetTurnstile()
    } finally {
      loading = false
    }
  }
</script>

<form class="flex flex-col gap-4" method="post" onsubmit={handleSubmit}>
  {#if formError}
    <Alert variant="destructive">
      <CircleAlertIcon class="size-4" />
      <AlertDescription>{formError}</AlertDescription>
    </Alert>
  {/if}

  <FormInput
    type="email"
    name="email"
    label="Email"
    placeholder="you@example.com"
    bind:value={emailValue}
    error={emailError}
    required
    autocomplete="email"
    onblur={validateEmail}
  />

  <FormInput
    type="password"
    name="password"
    label="Password"
    placeholder="Enter your password"
    bind:value={password}
    error={passwordError}
    required
    autocomplete="current-password"
    showPasswordToggle
    onblur={validatePassword}
  />

  <Button type="submit" disabled={loading} class="w-full">
    {#if loading}
      <span class="size-4 animate-spin rounded-full border-2 border-current border-r-transparent"></span>
    {/if}
    Sign in
  </Button>

  <OAuthButtons mode="login" />

  <p class="text-center text-sm text-muted-foreground">
    <a href="/forgot-password" class="font-medium text-primary hover:underline">Forgot your password?</a>
  </p>
</form>
