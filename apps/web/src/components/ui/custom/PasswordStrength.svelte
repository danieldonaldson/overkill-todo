<script lang="ts">
  interface Props {
    /** The password to evaluate */
    password: string
    /** Minimum required length (default: 8) */
    minLength?: number
  }

  let { password, minLength = 8 }: Props = $props()

  // Check requirements
  const hasMinLength = $derived(password.length >= minLength)
  const hasNumber = $derived(/\d/.test(password))
  const hasLetter = $derived(/[a-zA-Z]/.test(password))
  const hasSpecial = $derived(/[!@#$%^&*(),.?":{}|<>]/.test(password))

  // Overall strength (only length is required)
  const isValid = $derived(hasMinLength)

  // Strength score for visual feedback (0-4)
  const strengthScore = $derived.by(() => {
    let score = 0
    if (hasMinLength) score++
    if (hasNumber) score++
    if (hasLetter) score++
    if (hasSpecial) score++
    return score
  })

  // Only show when user has started typing
  const showIndicator = $derived(password.length > 0)

  // Text label so strength isn't conveyed by colour alone.
  const strengthLabels = ['', 'Weak', 'Fair', 'Good', 'Strong'] as const
  const strengthLabel = $derived(strengthLabels[strengthScore])
</script>

{#if showIndicator}
  <div class="password-strength">
    <div class="requirements">
      <div class="requirement" class:met={hasMinLength}>
        <span class="icon">{hasMinLength ? '✓' : '○'}</span>
        <span class="text">{minLength} characters</span>
      </div>

      {#if !isValid}
        <!-- Show hints when password is not yet valid -->
        <div class="hint">
          {#if !hasNumber && !hasLetter}
            Add letters or numbers
          {:else if password.length > 0 && password.length < minLength}
            {minLength - password.length} more characters needed
          {/if}
        </div>
      {:else}
        <!-- Show optional strength hints when valid -->
        <div class="optional-hints">
          {#if !hasNumber}
            <span class="optional">+ Add numbers for more security</span>
          {/if}
          {#if hasNumber && hasLetter}
            <span class="strong">Strong password</span>
          {/if}
        </div>
      {/if}
    </div>

    <!-- Visual strength bar, with a text label so meaning isn't colour-only -->
    <div class="strength-bar-row">
      <div class="strength-bar">
        <div
          class="strength-fill"
          class:weak={strengthScore === 1}
          class:fair={strengthScore === 2}
          class:good={strengthScore === 3}
          class:strong={strengthScore === 4}
          style="width: {(strengthScore / 4) * 100}%"
        ></div>
      </div>
      {#if strengthLabel}
        <span class="strength-label">{strengthLabel}</span>
      {/if}
    </div>
  </div>
{/if}

<style>
  @reference "../../../styles/global.css";

  .password-strength {
    @apply flex flex-col gap-2 mt-1;
  }

  .requirements {
    @apply flex flex-col gap-1;
  }

  .requirement {
    @apply flex items-center gap-2 text-sm;
    @apply text-muted-foreground;
    @apply transition-colors duration-150;
  }

  .requirement.met {
    @apply text-green-600 dark:text-green-400;
  }

  .requirement .icon {
    @apply text-xs w-4 text-center;
  }

  .requirement .text {
    @apply text-xs;
  }

  .hint {
    @apply text-xs text-muted-foreground ml-6;
  }

  .optional-hints {
    @apply flex flex-col gap-0.5 ml-6;
  }

  .optional {
    @apply text-xs text-muted-foreground;
  }

  .strong {
    @apply text-xs text-green-600 dark:text-green-400 font-medium;
  }

  .strength-bar-row {
    @apply flex items-center gap-2;
  }

  .strength-bar {
    @apply h-1 flex-1 bg-muted rounded-full overflow-hidden;
  }

  .strength-label {
    @apply text-xs text-muted-foreground tabular-nums;
  }

  .strength-fill {
    @apply h-full transition-all duration-300 rounded-full;
    @apply bg-muted-foreground;
  }

  .strength-fill.weak {
    @apply bg-red-500;
  }

  .strength-fill.fair {
    @apply bg-yellow-500;
  }

  .strength-fill.good {
    @apply bg-green-400;
  }

  .strength-fill.strong {
    @apply bg-green-600;
  }
</style>
