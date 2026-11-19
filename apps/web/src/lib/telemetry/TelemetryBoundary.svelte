<script lang="ts">
  // The root of every island (OT-006 Decision 6b; dashboard spec "A crashing
  // panel is contained"). An error while rendering or in an effect is reported
  // as `kind: boundary` with this island's `name`, and only this island is
  // replaced by a short message with "Try again", which rebuilds it in place.
  // The message never shows the error, its stack or a trace ID.
  //
  // `name` must be in backend/tests/fixtures/client_islands.json; the guard
  // test in islands.test.ts checks every `client:*` island uses one.
  import type { Snippet } from 'svelte'
  import { Button } from '$lib/components/ui/button'
  import { reportBoundaryError } from './index'
  import TestCrashHook from './TestCrashHook.svelte'

  // A constant per build: false in release images, so the hook never renders.
  const TEST_HOOKS = import.meta.env.DEV || import.meta.env.PUBLIC_TELEMETRY_TEST_HOOKS === 'true'

  interface Props {
    /** The island's allowlisted name, e.g. `SettingsPanel`. */
    name: string
    /** `none` for islands with no UI of their own (nothing to replace). */
    fallback?: 'message' | 'none'
    children: Snippet
  }

  let { name, fallback = 'message', children }: Props = $props()

  function onerror(error: unknown) {
    reportBoundaryError(name, error)
  }

  function reload() {
    location.reload()
  }
</script>

<svelte:boundary {onerror}>
  {#if TEST_HOOKS}
    <TestCrashHook {name} />
  {/if}
  {@render children()}

  <!-- eslint-disable-next-line @typescript-eslint/no-unused-vars -- Svelte passes the error first; it is reported in onerror and never shown -->
  {#snippet failed(_error, reset)}
    {#if fallback === 'message'}
      <div
        role="alert"
        data-telemetry-boundary={name}
        class="mx-auto flex w-full max-w-6xl min-w-0 flex-wrap items-center gap-x-4 gap-y-2 rounded-lg border border-border bg-card px-4 py-3 text-sm text-card-foreground"
      >
        <p class="min-w-0 flex-1 basis-48">This part of the page couldn't be shown.</p>
        <div class="flex flex-wrap items-center gap-3">
          <Button type="button" variant="outline" size="sm" onclick={reset}>Try again</Button>
          <button
            type="button"
            class="text-muted-foreground underline underline-offset-4 hover:text-foreground"
            onclick={reload}
          >
            Reload the page
          </button>
        </div>
      </div>
    {/if}
  {/snippet}
</svelte:boundary>
