<script lang="ts">
  // The `ModeWatcher` island (theme class on <html>), inside a boundary so an
  // error in it is reported and contained like any other island (OT-006). It
  // has no UI, so the boundary shows nothing in its place.
  import { ModeWatcher } from 'mode-watcher'
  import TelemetryBoundary from '$lib/telemetry/TelemetryBoundary.svelte'
</script>

<TelemetryBoundary name="ModeWatcher" fallback="none">
  <ModeWatcher />
</TelemetryBoundary>
