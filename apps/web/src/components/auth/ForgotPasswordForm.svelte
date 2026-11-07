<script lang="ts">
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert'
  import { Alert, AlertDescription } from '$lib/components/ui/alert'
  import { Button } from '$lib/components/ui/button'
  import { FormInput } from '$lib/components/ui/form-input'
  import Turnstile, { type TurnstileRef } from '../ui/custom/Turnstile.svelte'
  import { api } from '../../lib/api'
  import { navigate } from '../../lib/navigation'
  import { email, required, validate } from '../../lib/utils/validation'

  let emailValue = $state('')
  let turnstileToken = $state<string | null>(null)
  let emailError = $state<string | null>(null)
  let formError = $state<string | null>(null)
  let loading = $state(false)
  let turnstileRef = $state<TurnstileRef | undefined>(undefined)

  function validateEmail() {
    emailError = validate(emailValue, [required(), email()])
  }

  async function handleSubmit(event: SubmitEvent) {
    event.preventDefault()
    validateEmail()
    if (emailError) return

    loading = true
    formError = null

    try {
      await api.auth.forgotPassword({
        email: emailValue,
        turnstile_token: turnstileToken || undefined,
      })
      navigate(`/reset-password?email=${encodeURIComponent(emailValue)}`)
    } catch (error: unknown) {
      formError = error instanceof Error ? error.message : 'An unexpected error occurred'
      turnstileRef?.reset()
      turnstileToken = null
    } finally {
      loading = false
    }
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

  <p class="text-center text-sm text-muted-foreground">
    Enter your email address and we'll send you a code to reset your password.
  </p>

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

  <Turnstile bind:this={turnstileRef} onVerify={handleTurnstileVerify} />

  <Button type="submit" disabled={loading} class="w-full">
    {#if loading}
      <span class="size-4 animate-spin rounded-full border-2 border-current border-r-transparent"></span>
    {/if}
    Send reset code
  </Button>

  <p class="text-center text-sm text-muted-foreground">
    <a href="/login" class="font-medium text-primary hover:underline">Back to sign in</a>
  </p>
</form>
