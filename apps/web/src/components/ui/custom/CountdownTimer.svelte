<script lang="ts">
  import { onDestroy } from 'svelte'

  interface Props {
    /** Initial seconds to count down from */
    seconds: number
    /** Called when countdown reaches zero */
    oncomplete?: () => void
    /** Whether to auto-start the countdown (default: true) */
    autoStart?: boolean
    /** Text shown before the remaining time (default: "Resend in") */
    label?: string
  }

  let { seconds, oncomplete, autoStart = true, label = 'Resend in' }: Props = $props()

  // eslint-disable-next-line svelte/prefer-writable-derived -- needs mutable state for countdown tick
  let remainingSeconds = $state(0)

  // Initialize/sync from props when seconds changes
  $effect(() => {
    remainingSeconds = seconds
  })
  let intervalId: ReturnType<typeof setInterval> | null = null

  // Format as M:SS
  const formatted = $derived.by(() => {
    const mins = Math.floor(remainingSeconds / 60)
    const secs = remainingSeconds % 60
    return `${mins}:${secs.toString().padStart(2, '0')}`
  })

  const isComplete = $derived(remainingSeconds <= 0)

  function tick() {
    if (remainingSeconds > 0) {
      remainingSeconds--
      if (remainingSeconds <= 0) {
        stop()
        oncomplete?.()
      }
    }
  }

  function start() {
    if (intervalId) return
    intervalId = setInterval(tick, 1000)
  }

  function stop() {
    if (intervalId) {
      clearInterval(intervalId)
      intervalId = null
    }
  }

  export function reset(newSeconds?: number) {
    stop()
    remainingSeconds = newSeconds ?? seconds
    if (autoStart) {
      start()
    }
  }

  // Auto-start if enabled
  $effect(() => {
    if (autoStart && !isComplete) {
      start()
    }
    return () => stop()
  })

  onDestroy(() => {
    stop()
  })
</script>

{#if !isComplete}
  <span class="countdown">
    {#snippet prefix()}{label}
    {/snippet}
    {@render prefix()}
    <span class="time">{formatted}</span>
  </span>
{/if}

<style>
  @reference "../../../styles/global.css";

  .countdown {
    @apply text-sm text-muted-foreground;
  }

  .time {
    @apply font-mono font-medium tabular-nums;
  }
</style>
