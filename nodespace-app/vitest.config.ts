/// <reference types="vitest" />
/// <reference types="./src/tests/types/globals.d.ts" />
/// <reference types="./src/tests/types/vitest-env.d.ts" />
import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';

export default defineConfig({
  plugins: [sveltekit()],

  test: {
    include: ['src/tests/**/*.{test,spec}.{js,ts}'],
    environment: 'happy-dom', // Fast, modern DOM for Bun compatibility
    globals: true,
    setupFiles: ['src/tests/setup-svelte-mocks.ts', 'src/tests/setup.ts'],

    // Ensure proper global environment
    globalSetup: undefined,
    environmentOptions: {
      node: {
        // Ensure global object is available
        global: true
      }
    },

    // Simple coverage configuration
    coverage: {
      reporter: ['text', 'html'],
      exclude: ['node_modules/', 'src/tests/', '**/*.d.ts', '**/*.config.ts', 'build/', 'dist/'],
      thresholds: {
        lines: 80,
        functions: 80,
        branches: 75,
        statements: 80
      }
    },

    // Keep tests fast and simple
    testTimeout: 5000,
    hookTimeout: 5000
  }
});
