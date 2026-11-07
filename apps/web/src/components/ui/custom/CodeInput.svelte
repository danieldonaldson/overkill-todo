<script lang="ts">
  interface Props {
    /** Number of digits expected (default: 6) */
    length?: number
    /** Current value of the code */
    value?: string
    /** Whether the input is disabled */
    disabled?: boolean
    /** Error message to display */
    error?: string | null
    /** Called when a complete code is entered */
    oncomplete?: (code: string) => void
    /** Called when value changes */
    oninput?: (value: string) => void
  }

  let {
    length = 6,
    value = $bindable(''),
    disabled = false,
    error = null,
    oncomplete,
    oninput,
  }: Props = $props()

  let inputRef: HTMLInputElement

  // Strip non-digits and limit to length
  function sanitize(input: string): string {
    return input.replace(/\D/g, '').slice(0, length)
  }

  // Format for display (e.g., "123 456")
  const displayValue = $derived.by(() => {
    const clean = sanitize(value)
    if (length === 6 && clean.length > 3) {
      return clean.slice(0, 3) + ' ' + clean.slice(3)
    }
    return clean
  })

  function handleInput(e: Event) {
    const target = e.target as HTMLInputElement
    const newValue = sanitize(target.value)
    value = newValue

    // Update display value in input
    target.value = displayValue

    oninput?.(newValue)

    // Auto-submit when complete
    if (newValue.length === length) {
      oncomplete?.(newValue)
    }
  }

  function handleKeyDown(e: KeyboardEvent) {
    // Never interfere with keyboard shortcuts (paste, select-all, etc.) —
    // anything pressed with a modifier is a command, not character input.
    // This keeps Ctrl/Cmd+V, Cmd+A, Shift+Insert, etc. working. The paste
    // and input handlers sanitize the result anyway.
    if (e.ctrlKey || e.metaKey || e.altKey) {
      return
    }

    // Navigation/editing keys (Backspace, Tab, arrows, …) report multi-char
    // names; only single printable characters can be non-digit input.
    if (e.key.length !== 1) {
      return
    }

    // Block bare non-digit characters
    if (!/^\d$/.test(e.key)) {
      e.preventDefault()
    }
  }

  function handlePaste(e: ClipboardEvent) {
    e.preventDefault()
    const pasted = e.clipboardData?.getData('text') || ''
    const newValue = sanitize(pasted)
    value = newValue

    if (inputRef) {
      inputRef.value = displayValue
    }

    oninput?.(newValue)

    if (newValue.length === length) {
      oncomplete?.(newValue)
    }
  }

  export function focus() {
    inputRef?.focus()
  }

  export function clear() {
    value = ''
    if (inputRef) {
      inputRef.value = ''
    }
  }
</script>

<div class="code-input-wrapper">
  <input
    bind:this={inputRef}
    type="text"
    inputmode="numeric"
    autocomplete="one-time-code"
    maxlength={length + 1}
    placeholder="000 000"
    value={displayValue}
    {disabled}
    class="code-input"
    class:has-error={error}
    oninput={handleInput}
    onkeydown={handleKeyDown}
    onpaste={handlePaste}
    aria-label="Verification code"
    aria-invalid={error ? 'true' : undefined}
    aria-describedby={error ? 'code-error' : undefined}
  />
  {#if error}
    <span id="code-error" role="alert" aria-live="polite" class="error-text">{error}</span>
  {/if}
</div>

<style>
  @reference "../../../styles/global.css";

  .code-input-wrapper {
    @apply flex flex-col gap-1;
  }

  .code-input {
    @apply w-full h-14 px-4 text-2xl font-mono text-center tracking-[0.5em];
    @apply text-foreground bg-card;
    @apply border border-input rounded-lg;
    @apply transition-colors duration-150;
    @apply placeholder:text-muted-foreground/60;
    @apply placeholder:tracking-[0.5em];
  }

  .code-input:focus {
    @apply outline-none border-ring;
    @apply ring-3 ring-ring/30;
  }

  .code-input:disabled {
    @apply bg-muted cursor-not-allowed opacity-70;
  }

  .code-input.has-error {
    @apply border-destructive;
  }

  .code-input.has-error:focus {
    @apply ring-destructive/30;
  }

  .error-text {
    @apply text-xs text-destructive text-center;
  }
</style>
