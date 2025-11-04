/// <reference types="vitest" />
import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  plugins: [sveltekit()],

  // Configure path aliases
  resolve: {
    alias: {
      $lib: path.resolve(__dirname, 'src/lib'),
      $app: path.resolve(__dirname, 'node_modules/@sveltejs/kit/src/runtime/app')
    }
  },

  test: {
    // Enable Vitest Browser Mode
    browser: {
      enabled: true,
      name: 'chromium',
      provider: 'playwright',
      headless: true,
      // Enable screenshotting for debugging failures (helps identify issues quickly)
      screenshotFailures: true
    },

    // Only run browser integration tests
    include: ['src/tests/browser/**/*.{test,spec}.{js,ts}'],

    // Setup files for browser tests
    setupFiles: ['src/tests/setup-browser.ts'],

    // Longer timeouts for browser tests
    testTimeout: 30000,
    hookTimeout: 30000,

    // Run browser tests sequentially to prevent interference
    sequence: {
      concurrent: false,
      shuffle: false
    },

    // Use forks pool for isolation
    pool: 'forks',
    poolOptions: {
      forks: {
        singleFork: true
      }
    }
  }
});
