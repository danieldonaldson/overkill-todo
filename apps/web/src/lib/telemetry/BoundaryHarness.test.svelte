<script lang="ts">
  import BoundaryThrower from './BoundaryThrower.test.svelte'
  import TelemetryBoundary from './TelemetryBoundary.svelte'

  let {
    control,
    fallback = 'message',
  }: { control: { fail: boolean }; fallback?: 'message' | 'none' } = $props()
</script>

<TelemetryBoundary name="SettingsPanel" {fallback}>
  <BoundaryThrower {control} />
</TelemetryBoundary>
<a href="/dashboard">Outside the boundary</a>
