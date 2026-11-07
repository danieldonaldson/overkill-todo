<script lang="ts">
  import type { HTMLInputAttributes } from 'svelte/elements'
  import EyeIcon from '@lucide/svelte/icons/eye'
  import EyeOffIcon from '@lucide/svelte/icons/eye-off'
  import { Input } from '$lib/components/ui/input'
  import { Label } from '$lib/components/ui/label'
  import { cn } from '$lib/utils'

  interface Props {
    type?: 'text' | 'email' | 'password' | 'number' | 'date' | 'tel'
    name: string
    label?: string
    placeholder?: string
    value?: string | number
    error?: string | null
    required?: boolean
    disabled?: boolean
    autocomplete?: HTMLInputAttributes['autocomplete']
    inputmode?: 'none' | 'text' | 'decimal' | 'numeric' | 'tel' | 'search' | 'email' | 'url'
    min?: number | string
    max?: number | string
    step?: number | string
    maxlength?: number
    showPasswordToggle?: boolean
    class?: string
    oninput?: (e: Event) => void
    onblur?: (e: FocusEvent) => void
  }

  let {
    type = 'text',
    name,
    label,
    placeholder,
    value = $bindable(''),
    error = null,
    required = false,
    disabled = false,
    autocomplete,
    inputmode,
    min,
    max,
    step,
    maxlength,
    showPasswordToggle = false,
    class: className,
    oninput,
    onblur,
  }: Props = $props()

  let passwordVisible = $state(false)

  const id = $derived(`input-${name}`)
  const inputType = $derived(type === 'password' && passwordVisible ? 'text' : type)

  function togglePasswordVisibility() {
    passwordVisible = !passwordVisible
  }
</script>

<div class={cn('flex flex-col gap-1.5', className)}>
  {#if label}
    <Label for={id} class="text-sm font-medium">
      {label}
      {#if required}<span class="text-destructive ml-1">*</span>{/if}
    </Label>
  {/if}
  <div class="relative">
    {#if type === 'password' && showPasswordToggle}
      <button
        type="button"
        class="absolute left-2.5 top-1/2 -translate-y-1/2 flex items-center justify-center size-6 p-0 border-none bg-transparent rounded-sm cursor-pointer text-muted-foreground hover:text-foreground transition-colors"
        onclick={togglePasswordVisibility}
        aria-label={passwordVisible ? 'Hide password' : 'Show password'}
        tabindex={-1}
      >
        {#if passwordVisible}
          <EyeOffIcon class="size-4" />
        {:else}
          <EyeIcon class="size-4" />
        {/if}
      </button>
    {/if}
    <Input
      {id}
      type={inputType}
      {name}
      {placeholder}
      bind:value
      {required}
      {disabled}
      {autocomplete}
      {inputmode}
      {min}
      {max}
      {step}
      {maxlength}
      class={cn(
        type === 'password' && showPasswordToggle && 'pl-10',
        error && 'border-destructive'
      )}
      {oninput}
      {onblur}
      aria-invalid={error ? 'true' : undefined}
      aria-describedby={error ? `${id}-error` : undefined}
    />
  </div>
  {#if error}
    <span id="{id}-error" role="alert" aria-live="polite" class="text-xs text-destructive">
      {error}
    </span>
  {/if}
</div>
