<script lang="ts">
  import BoundaryThrower from './BoundaryThrower.test.svelte'
  import type { IslandTimer } from './island-timing'
  import TelemetryBoundary from './TelemetryBoundary.svelte'

  let {
    control,
    fallback = 'message',
    timing,
  }: { control: { fail: boolean }; fallback?: 'message' | 'none'; timing?: IslandTimer } = $props()
</script>

<TelemetryBoundary name="SettingsPanel" {fallback} {timing}>
  <BoundaryThrower {control} />
</TelemetryBoundary>
<a href="/dashboard">Outside the boundary</a>
