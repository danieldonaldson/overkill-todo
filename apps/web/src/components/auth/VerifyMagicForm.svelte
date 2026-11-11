<script lang="ts">
  import { onMount } from 'svelte'
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert'
  import { Alert, AlertDescription } from '$lib/components/ui/alert'
  import { Button } from '$lib/components/ui/button'
  import { api } from '../../lib/api'
  import { saveSignupHandoff } from '../../lib/auth/signup-handoff'
  import { navigate } from '../../lib/navigation'

  // The token arrives in the URL fragment (`#token=…`), which browsers never
  // send to a server. Nothing happens until the user presses the button: mail
  // scanners fetch links but don't click, so they can't use the token up.
  let token = $state('')
  let formError = $state<string | null>(null)
  let loading = $state(false)

  onMount(() => {
    token = new URLSearchParams(window.location.hash.slice(1)).get('token') ?? ''
    // Drop the token from the address bar (and so from history) right away.
    if (window.location.hash) {
      history.replaceState(null, '', window.location.pathname + window.location.search)
    }
    if (!token) {
      formError = 'This link is incomplete. Open it again from your email, or request a new code.'
    }
  })

  async function handleConfirm() {
    if (!token || loading) return
    loading = true
    formError = null

    try {
      const response = await api.auth.verifyMagicLink(token)
      if (response.signup_token && response.user) {
        saveSignupHandoff({ token: response.signup_token, email: response.user.email })
        navigate('/set-password')
      } else {
        // Account already had a password: logged in via HttpOnly cookies.
        navigate('/dashboard')
      }
    } catch (error: unknown) {
      if (error && typeof error === 'object' && 'error_code' in error) {
        const authError = error as { error?: string; error_code: string }
        if (authError.error_code === 'ALREADY_VERIFIED') {
          navigate('/login?message=email_verified')
          return
        }
        formError = authError.error || 'This link could not be used. Request a new code.'
      } else {
        formError = error instanceof Error ? error.message : 'An unexpected error occurred'
      }
    } finally {
      loading = false
    }
  }
</script>

<div class="flex flex-col gap-4">
  {#if formError}
    <Alert variant="destructive">
      <CircleAlertIcon class="size-4" />
      <AlertDescription>{formError}</AlertDescription>
    </Alert>
    <p class="text-center text-sm text-muted-foreground">
      <a href="/register" class="font-medium text-primary hover:underline">Request a new code</a>
    </p>
  {:else}
    <p class="text-center text-sm text-muted-foreground">
      Press the button to confirm this is your email address.
    </p>
    <Button onclick={handleConfirm} disabled={loading || !token} class="w-full">
      {#if loading}
        <span class="size-4 animate-spin rounded-full border-2 border-current border-r-transparent"
        ></span>
      {/if}
      Confirm my email
    </Button>
  {/if}
</div>
