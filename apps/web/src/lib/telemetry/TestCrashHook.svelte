<script lang="ts">
  // Test hook (OT-006): throws while rendering when `window.__otCrashIsland`
  // names this island, so e2e flows and the local check can crash any island on
  // purpose. Rendered only in `astro dev` and in builds with
  // PUBLIC_TELEMETRY_TEST_HOOKS=true (`just e2e`, CI's e2e job); never in a
  // release image. See `TEST_HOOKS` in TelemetryBoundary.svelte.
  let { name }: { name: string } = $props()

  function crashIfAsked(island: string) {
    const asked = (globalThis as { __otCrashIsland?: unknown }).__otCrashIsland
    if (asked === island) throw new Error(`Deliberate test crash in ${island}`)
  }
  crashIfAsked(name)
</script>
