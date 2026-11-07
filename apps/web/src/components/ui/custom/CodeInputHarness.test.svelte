<script lang="ts">
  import CodeInput from './CodeInput.svelte'

  let { value = $bindable('') }: { value?: string } = $props()
  let ref = $state<CodeInput | undefined>(undefined)
</script>

<CodeInput bind:this={ref} bind:value />
<button type="button" data-testid="do-focus" onclick={() => ref?.focus()}>focus</button>
<button type="button" data-testid="do-clear" onclick={() => ref?.clear()}>clear</button>
