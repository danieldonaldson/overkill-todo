<script lang="ts">
  import { cn } from '$lib/utils.js'
  import Loader2Icon from '@lucide/svelte/icons/loader-2'
  import type { ComponentProps } from 'svelte'

  type Size = 'sm' | 'md' | 'lg' | number

  interface Props extends Omit<ComponentProps<typeof Loader2Icon>, 'size'> {
    size?: Size
  }

  let { class: className, size = 'md', ...restProps }: Props = $props()

  const sizeClasses: Record<string, string> = {
    sm: 'size-4',
    md: 'size-6',
    lg: 'size-8',
  }

  const sizeClass = $derived(typeof size === 'string' ? sizeClasses[size] || 'size-6' : '')
  const sizePixels = $derived(typeof size === 'number' ? size : undefined)
</script>

<Loader2Icon
  role="status"
  aria-label="Loading"
  class={cn('animate-spin', sizeClass, className)}
  size={sizePixels}
  {...restProps}
/>
