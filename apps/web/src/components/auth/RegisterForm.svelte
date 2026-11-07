<script lang="ts">
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert'
  import { Alert, AlertDescription } from '$lib/components/ui/alert'
  import { Button } from '$lib/components/ui/button'
  import { FormInput } from '$lib/components/ui/form-input'
  import { api } from '../../lib/api'
  import { navigate } from '../../lib/navigation'
  import { email, required, validate } from '../../lib/utils/validation'
  import OAuthButtons from './OAuthButtons.svelte'

  // See LoginForm: the Turnstile widget belongs to the caller, not the form.
  interface Props {
    emailValue?: string
    turnstileToken?: string | null
    resetTurnstile?: () => void
  }

  let { emailValue = $bindable(''), turnstileToken = null, resetTurnstile = () => {} }: Props = $props()

  let emailError = $state<string | null>(null)
  let formError = $state<string | null>(null)
  let loading = $state(false)

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
      await api.auth.register({
        email: emailValue,
        turnstile_token: turnstileToken || undefined,
      })
      navigate(`/verify-email?email=${encodeURIComponent(emailValue)}`)
    } catch (error: unknown) {
      formError = error instanceof Error ? error.message : 'An unexpected error occurred'
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

  <Button type="submit" disabled={loading} class="w-full">
    {#if loading}
      <span class="size-4 animate-spin rounded-full border-2 border-current border-r-transparent"></span>
    {/if}
    Continue
  </Button>

  <OAuthButtons mode="register" />
</form>
