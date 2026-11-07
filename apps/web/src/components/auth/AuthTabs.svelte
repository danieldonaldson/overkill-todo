<script lang="ts">
  import { Tabs, TabsContent, TabsList, TabsTrigger } from '$lib/components/ui/tabs'
  import Turnstile, { type TurnstileRef } from '../ui/custom/Turnstile.svelte'
  import LoginForm from './LoginForm.svelte'
  import RegisterForm from './RegisterForm.svelte'

  type Mode = 'login' | 'register'

  interface Props {
    initial?: Mode
  }

  let { initial = 'login' }: Props = $props()

  // The page route seeds the tab once; after that the tab owns the URL, not the reverse.
  // svelte-ignore state_referenced_locally
  let mode = $state<string>(initial)

  const copy = {
    login: {
      path: '/login',
      heading: 'Welcome back',
      description: 'Access your dashboard and automations.',
    },
    register: {
      path: '/register',
      heading: 'Create your account',
      description: "Start with your email — we'll send you a verification code.",
    },
  } as const

  const active = $derived(copy[mode as Mode])

  // Turnstile and the typed email live out here, above the panels that mount and
  // unmount, so toggling tabs neither re-runs the challenge nor clears the field.
  let turnstileRef = $state<TurnstileRef | undefined>(undefined)
  let turnstileToken = $state<string | null>(null)
  let emailValue = $state('')

  function resetTurnstile() {
    // Clear first: in dev, reset() re-issues the token synchronously.
    turnstileToken = null
    turnstileRef?.reset()
  }

  // Not $state: the first effect run is the mount, where the URL already matches
  // the initial tab and rewriting it would drop any query string (/login?session=expired).
  let mounted = false

  // Switching tabs rewrites the address bar so a refresh or a copied link lands
  // on the panel the user is looking at. replaceState, not pushState: Back
  // should leave the auth screen, not walk back through tab switches.
  $effect(() => {
    const { path } = active
    if (!mounted) {
      mounted = true
      return
    }
    history.replaceState(history.state, '', path)
  })
</script>

<Tabs bind:value={mode} class="gap-0">
  <TabsList>
    <TabsTrigger value="login">Sign in</TabsTrigger>
    <TabsTrigger value="register">Create account</TabsTrigger>
  </TabsList>

  <div class="mt-6 mb-6 text-center">
    <h1 class="font-display text-2xl font-semibold tracking-tight">{active.heading}</h1>
    <p class="mt-2 text-sm text-muted-foreground">{active.description}</p>
  </div>

  <Turnstile bind:this={turnstileRef} onVerify={(token) => (turnstileToken = token || null)} />

  <!--
    bits-ui renders every panel and hides the inactive ones, so the panels gate
    their own contents: otherwise both forms would stay mounted and a hidden
    password field would sit in the DOM for autofill to find on the
    create-account tab.
  -->
  <TabsContent value="login">
    {#if mode === 'login'}
      <LoginForm bind:emailValue {turnstileToken} {resetTurnstile} />
      <p class="mt-4 text-center text-sm text-muted-foreground">
        New here?
        <button
          type="button"
          class="font-medium text-primary hover:underline"
          onclick={() => (mode = 'register')}>Create account</button
        >
      </p>
    {/if}
  </TabsContent>

  <TabsContent value="register">
    {#if mode === 'register'}
      <RegisterForm bind:emailValue {turnstileToken} {resetTurnstile} />
      <p class="mt-4 text-center text-sm text-muted-foreground">
        Already have an account?
        <button
          type="button"
          class="font-medium text-primary hover:underline"
          onclick={() => (mode = 'login')}>Sign in</button
        >
      </p>
    {/if}
  </TabsContent>
</Tabs>
