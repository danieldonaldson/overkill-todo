<script lang="ts" module>
  export interface TurnstileRef {
    reset: () => void
  }
</script>

<script lang="ts">
  import { onMount } from 'svelte'
  import { TURNSTILE_SITE_KEY } from '../../../lib/config'

  let {
    onVerify,
    forceProduction = false,
  }: { onVerify: (token: string) => void; forceProduction?: boolean } = $props()

  let container = $state<HTMLDivElement | undefined>(undefined)
  let widgetId: string | null = null

  // Check if we're running in development mode (must be computed client-side for static builds)
  // forceProduction prop allows tests to override this check
  const isDev = $derived(
    !forceProduction &&
      typeof window !== 'undefined' &&
      (window.location.hostname === 'localhost' || window.location.hostname === '127.0.0.1')
  )

  // Expose reset function for external callers
  export function reset() {
    if (isDev) {
      // In dev mode, just re-verify with the dev token
      onVerify('dev-mode-skip')
      return
    }
    if (window.turnstile && widgetId) {
      window.turnstile.reset(widgetId)
    }
  }

  function renderWidget() {
    if (!window.turnstile || !container) return

    widgetId = window.turnstile.render(container, {
      sitekey: TURNSTILE_SITE_KEY,
      // Stay invisible and issue the token silently; Cloudflare only paints the
      // widget for the minority of visitors it wants to actually challenge.
      appearance: 'interaction-only',
      callback: onVerify,
      'error-callback': () => {
        // Verification failed — clear any previously issued token so the parent
        // doesn't submit a stale one.
        onVerify('')
      },
      'expired-callback': () => {
        // Token expired, user needs to re-verify
        onVerify('')
      },
      theme: 'auto',
    })
  }

  function loadScript() {
    // Check if already loaded
    if (window.turnstile) {
      renderWidget()
      return
    }

    // Push into a shared queue so multiple concurrent components all get rendered
    type Cb = () => void
    const queue: Cb[] = ((window as Record<string, unknown>).__turnstileQueue ??= []) as Cb[]
    queue.push(renderWidget)

    if (document.querySelector('script[src*="turnstile"]')) {
      // Script is already loading — callback queued, nothing else to do
      return
    }

    // First caller sets up the onload handler to drain the queue
    window.onTurnstileLoad = () => {
      const pending = ((window as Record<string, unknown>).__turnstileQueue ?? []) as Cb[]
      ;(window as Record<string, unknown>).__turnstileQueue = []
      for (const cb of pending) cb()
    }

    const script = document.createElement('script')
    script.src = 'https://challenges.cloudflare.com/turnstile/v0/api.js?onload=onTurnstileLoad'
    script.async = true
    script.defer = true
    document.head.appendChild(script)
  }

  onMount(() => {
    if (isDev) {
      // In dev mode, auto-verify with empty token (backend should skip validation in dev)
      onVerify('dev-mode-skip')
      return
    }

    loadScript()

    return () => {
      if (window.turnstile && widgetId) {
        window.turnstile.remove(widgetId)
      }
    }
  })
</script>

{#if !isDev}
  <div class="turnstile-container">
    <div bind:this={container}></div>
  </div>
{/if}

<style>
  /* No min-height: under `interaction-only` the widget is usually zero-height,
     and reserving space for it would leave a gap in the form. */
  .turnstile-container {
    display: flex;
    justify-content: center;
  }
</style>
