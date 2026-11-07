<script lang="ts">
  import { getContext, onMount } from 'svelte'
  import HomeIcon from '@lucide/svelte/icons/home'
  import SettingsIcon from '@lucide/svelte/icons/settings'
  import ShieldIcon from '@lucide/svelte/icons/shield'
  import ThemeToggle from '../ui/custom/ThemeToggle.svelte'
  import { AUTH_CONTEXT_KEY, type AuthContext } from '../../lib/auth/cookie'
  import { isAdminRole } from '../../lib/types'

  // `pathname` is injectable for tests; in the app it tracks the real location,
  // updated on Astro client-side navigations (astro:page-load).
  let { pathname: pathnameProp }: { pathname?: string } = $props()

  // Rendered inside AuthProvider (see DashboardShell), so the auth context is
  // available. The Admin link is shown only to admins / super-admins; the
  // backend enforces access regardless.
  const auth = getContext<AuthContext>(AUTH_CONTEXT_KEY)
  const isAdmin = $derived(isAdminRole(auth?.user?.role))

  let currentPath = $state('')

  // Test-injected path wins and stays in sync; the app path is tracked below.
  $effect(() => {
    if (pathnameProp !== undefined) currentPath = pathnameProp
  })

  onMount(() => {
    if (pathnameProp !== undefined) return
    const sync = () => (currentPath = window.location.pathname)
    sync()
    document.addEventListener('astro:page-load', sync)
    return () => document.removeEventListener('astro:page-load', sync)
  })

  const baseLinks = [
    { href: '/dashboard', label: 'Home', icon: HomeIcon },
    { href: '/dashboard/settings', label: 'Settings', icon: SettingsIcon },
  ]
  const links = $derived(
    isAdmin
      ? [...baseLinks, { href: '/dashboard/admin', label: 'Admin', icon: ShieldIcon }]
      : baseLinks,
  )

  // Home is an exact match (every route starts with /dashboard); the others own
  // their subtree so nested pages keep their tab lit.
  const isActive = (href: string) =>
    href === '/dashboard' ? currentPath === '/dashboard' : currentPath.startsWith(href)
</script>

<nav aria-label="Dashboard" class="flex flex-1 items-center justify-end gap-2">
  <!-- Segmented pill: active = filled pill (aria-current), inactive = ghost.
       Icon-only below sm keeps it thumb-friendly; the label stays as the
       accessible name via sr-only. -->
  <ul class="flex items-center gap-1 rounded-full bg-muted/60 p-1">
    {#each links as link (link.href)}
      {@const active = isActive(link.href)}
      <li>
        <a
          href={link.href}
          aria-current={active ? 'page' : undefined}
          class="inline-flex h-9 items-center gap-2 rounded-full px-3 text-sm font-medium no-underline transition-all duration-[var(--motion-fast)] ease-out hover:no-underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background {active
            ? 'bg-primary text-primary-foreground shadow-xs'
            : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground'}"
        >
          <link.icon class="size-4" aria-hidden="true" />
          <span class="sr-only sm:not-sr-only">{link.label}</span>
        </a>
      </li>
    {/each}
  </ul>

  <ThemeToggle />
</nav>
