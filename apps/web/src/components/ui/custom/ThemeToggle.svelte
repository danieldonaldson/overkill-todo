<script lang="ts">
  // Theme toggle backed by mode-watcher (toggles html.dark + persists choice).
  import { mode, toggleMode } from 'mode-watcher'
</script>

<button
  type="button"
  class="theme-toggle"
  onclick={toggleMode}
  aria-label="Toggle theme"
  title={mode.current === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
>
  {#if mode.current === 'dark'}
    <!-- Sun icon -->
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <circle cx="12" cy="12" r="4"></circle>
      <path d="M12 2v2"></path>
      <path d="M12 20v2"></path>
      <path d="m4.93 4.93 1.41 1.41"></path>
      <path d="m17.66 17.66 1.41 1.41"></path>
      <path d="M2 12h2"></path>
      <path d="M20 12h2"></path>
      <path d="m6.34 17.66-1.41 1.41"></path>
      <path d="m19.07 4.93-1.41 1.41"></path>
    </svg>
  {:else}
    <!-- Moon icon -->
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"></path>
    </svg>
  {/if}
</button>

<style>
  @reference '../../../styles/global.css';

  .theme-toggle {
    @apply flex h-9 w-9 items-center justify-center rounded-lg;
    @apply text-muted-foreground hover:bg-accent hover:text-accent-foreground;
    @apply cursor-pointer border-0 bg-transparent transition-colors duration-150;
  }

  .theme-toggle:focus-visible {
    @apply outline-ring outline-2 outline-offset-2;
  }
</style>
