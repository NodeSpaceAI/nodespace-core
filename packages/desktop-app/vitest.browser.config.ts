/// <reference types="vitest" />
import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { nodespaceExtensions } from './vite-plugins/nodespace-extensions.js';
import {
  EXTENSIONS_ENV,
  resolveExtensionsEntry
} from './vite-plugins/nodespace-extensions-entry.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/**
 * Browser-mode tier (Chromium / Blink via Playwright).
 *
 * ENGINE-AGNOSTIC CONTRACT: the tests under `src/tests/browser/**` must exercise
 * only standard DOM behavior that any modern engine implements the same way —
 * focus/blur, `document.activeElement`, textarea selection, keyboard events,
 * non-zero `getBoundingClientRect`, viewport-based positioning math. The exact
 * same suite is run against WebKit (the shipping WKWebView engine) by
 * `vitest.webkit.config.ts`, and both must pass.
 *
 * Do NOT add engine-specific assertions here (e.g. pixel-exact layout values,
 * Chromium-only APIs, or behavior that differs between Blink and WebKit). Such a
 * test would pass under Chromium and fail the WebKit smoke tier — or, worse,
 * silently validate the wrong engine for the app users actually run.
 *
 * LOCKSTEP TWIN: `vitest.webkit.config.ts` must stay identical to this file
 * except for `browser.name`. The engine-agnostic contract depends on both
 * running the same suite the same way — change one (timeouts, setup files,
 * pool, include glob), change the other.
 */
export default defineConfig({
  plugins: [
    nodespaceExtensions({
      root: __dirname,
      entry: resolveExtensionsEntry(process.env[EXTENSIONS_ENV], __dirname)
    }),
    sveltekit()
  ],

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
