<script lang="ts">
  import { onMount } from 'svelte'
  import CheckIcon from '@lucide/svelte/icons/check'
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert'
  import { Alert, AlertDescription } from '$lib/components/ui/alert'
  import { Button } from '$lib/components/ui/button'
  import { FormInput } from '$lib/components/ui/form-input'
  import PasswordStrength from '../ui/custom/PasswordStrength.svelte'
  import { api } from '../../lib/api'
  import { navigate } from '../../lib/navigation'
  import { minLength, required, validate } from '../../lib/utils/validation'

  let { token: propToken = '', email: propEmail = '' }: { token?: string; email?: string } = $props()

  const token = $derived(propToken || getUrlParam('token'))
  const email = $derived(propEmail || getUrlParam('email'))
  const passwordsMatch = $derived(confirmPassword.length > 0 && confirmPassword === password)

  let password = $state('')
  let confirmPassword = $state('')
  let passwordError = $state<string | null>(null)
  let confirmPasswordError = $state<string | null>(null)
  let formError = $state<string | null>(null)
  let loading = $state(false)

  onMount(() => {
    if (!propToken && !new URLSearchParams(window.location.search).get('token')) {
      navigate('/register')
    }
  })

  function getUrlParam(name: string): string {
    if (typeof window === 'undefined') return ''
    return new URLSearchParams(window.location.search).get(name) || ''
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

  async function handleSubmit(event: SubmitEvent) {
    event.preventDefault()
    validatePassword()
    validateConfirmPassword()

    if (passwordError || confirmPasswordError) return

    if (!token) {
      formError = 'Missing signup token. Please start the registration process again.'
      return
    }

    loading = true
    formError = null

    try {
      await api.auth.completeSignup({
        signup_token: token,
        email,
        password,
      })
      navigate('/dashboard')
    } catch (error: unknown) {
      if (error && typeof error === 'object' && 'error_code' in error) {
        const authError = error as { error?: string; error_code: string }
        if (authError.error_code === 'TOKEN_EXPIRED') {
          formError = 'Your signup session has expired. Please register again.'
        } else if (authError.error_code === 'TOKEN_INVALID') {
          formError = 'Invalid signup token. Please register again.'
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
</script>

<form class="flex flex-col gap-4" method="post" onsubmit={handleSubmit}>
  {#if formError}
    <Alert variant="destructive">
      <CircleAlertIcon class="size-4" />
      <AlertDescription>{formError}</AlertDescription>
    </Alert>
  {/if}

  <p class="text-center text-sm text-muted-foreground">Create a password to secure your account.</p>

  <FormInput type="email" name="email" label="Email" value={email} disabled autocomplete="email" />

  <div class="flex flex-col gap-1">
    <FormInput
      type="password"
      name="password"
      label="Password"
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
      label="Confirm password"
      placeholder="Confirm your password"
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
    {loading ? 'Creating account...' : 'Create account'}
  </Button>

  <p class="text-center text-sm text-muted-foreground">
    <a href="/register" class="font-medium text-primary hover:underline">Start over</a>
  </p>
</form>
