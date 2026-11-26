import { defineConfig } from 'vitest/config'

// Model replays (OT-013 design D13): Node-mode Vitest over the ITF traces that
// scripts/models.sh exports into MODEL_TRACES_DIR. Run by `just models`, not
// by `just test-web`, whose config includes only *.test.ts.
export default defineConfig({
  test: {
    include: ['src/**/*.replay.ts'],
    environment: 'node',
    testTimeout: 600_000,
  },
  resolve: {
    alias: {
      '@': new URL('./src', import.meta.url).pathname,
      $lib: new URL('./src/lib', import.meta.url).pathname,
    },
  },
})
