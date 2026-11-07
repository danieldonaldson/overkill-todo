<script lang="ts">
  import AuthProvider from '../auth/AuthProvider.svelte'
  import DashboardNav from './DashboardNav.svelte'

  let { children }: { children?: import('svelte').Snippet } = $props()
</script>

<AuthProvider>
  <div class="min-h-screen bg-background text-foreground">
    <header class="sticky top-0 z-50 border-b border-border bg-background/95 backdrop-blur">
      <div class="mx-auto flex h-16 w-full max-w-6xl items-center gap-6 px-4">
        <a href="/dashboard" class="flex items-center gap-2 text-lg font-semibold text-primary">
          <span
            class="flex size-8 items-center justify-center rounded-md bg-primary text-sm font-bold text-primary-foreground"
            aria-hidden="true"
          >
            t
          </span>
          <span>app</span>
        </a>

        <DashboardNav />
      </div>
    </header>

    <main class="mx-auto w-full max-w-6xl px-4 py-6">
      {@render children?.()}
    </main>
  </div>
</AuthProvider>
