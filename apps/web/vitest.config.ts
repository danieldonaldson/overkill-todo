import { svelte } from '@sveltejs/vite-plugin-svelte'
import { playwright } from '@vitest/browser-playwright'
import { defineConfig } from 'vitest/config'

export default defineConfig({
  plugins: [
    svelte({
      prebundleSvelteLibraries: true,
    }),
  ],
  test: {
    include: ['src/**/*.{test,spec}.{js,ts}'],
    setupFiles: ['vitest-browser-svelte', './vitest.setup.ts'],
    browser: {
      enabled: true,
      provider: playwright(),
      instances: [{ browser: 'chromium' }],
      headless: true,
    },
    coverage: {
      provider: 'v8',
      reportsDirectory: './coverage',
      reporter: ['text', 'html', 'lcov'],
      include: ['src/**/*.{js,ts,svelte}'],
      exclude: [
        'src/test-utils/**',
        'src/**/*.d.ts',
        // Vendored shadcn-svelte primitive shells: prop-forwarding boilerplate
        // (cn() class merge + {@render children}) with no project logic. Our
        // custom wrappers (form-input, button, input, spinner) stay included.
        'src/lib/components/ui/alert/**',
        'src/lib/components/ui/card/**',
        'src/lib/components/ui/dropdown-menu/**',
        'src/lib/components/ui/input-otp/**',
        'src/lib/components/ui/label/**',
        'src/lib/components/ui/sonner/**',
      ],
    },
    testTimeout: 10000,
  },
  optimizeDeps: {
    exclude: ['@lucide/svelte', '@testing-library/svelte-core', 'bits-ui', 'mode-watcher', 'svelte-sonner'],
  },
  resolve: {
    conditions: ['browser', 'development'],
    alias: {
      '@': new URL('./src', import.meta.url).pathname,
      '$lib': new URL('./src/lib', import.meta.url).pathname,
      'astro:transitions/client': new URL(
        './src/test-utils/astro-transitions-mock.ts',
        import.meta.url
      ).pathname,
    },
  },
})
