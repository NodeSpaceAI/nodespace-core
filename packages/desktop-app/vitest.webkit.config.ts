/// <reference types="vitest" />
import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/**
 * WebKit (JavaScriptCore) smoke tier — the engine family of the shipping
 * WKWebView desktop app.
 *
 * This runs the SAME engine-agnostic browser suite (`src/tests/browser/**`) as
 * `vitest.browser.config.ts`, but against Playwright's bundled WebKit build
 * instead of Chromium. Chromium (Blink) and WebKit (JavaScriptCore) are
 * different engines; the desktop app ships WebKit, so a suite that only ever
 * runs under Chromium can pass here yet break in the real app.
 *
 * The value is the delta: any browser test that passes under Chromium but fails
 * under WebKit is a real, WebKit-specific finding. Passing under both engines is
 * the proof that the browser tier is genuinely engine-agnostic.
 *
 * This tier is NIGHTLY and NON-BLOCKING — it is deliberately kept out of
 * `test:all` and the merge gate (see `.github/workflows/nightly-webkit.yml`).
 *
 * Run with: bun run test:webkit
 * Requires one-time setup: bunx playwright install webkit
 *
 * LOCKSTEP TWIN: this file must stay identical to `vitest.browser.config.ts`
 * except for `browser.name` (webkit vs chromium). If you change a timeout,
 * setup file, pool option, or include glob in one, change it in the other —
 * the engine-agnostic contract depends on both running the same suite the
 * same way.
 */
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
    // Enable Vitest Browser Mode against WebKit (the shipping WKWebView engine).
    browser: {
      enabled: true,
      name: 'webkit',
      provider: 'playwright',
      headless: true,
      // Enable screenshotting for debugging failures (helps identify issues quickly)
      screenshotFailures: true
    },

    // Run the same engine-agnostic browser suite as vitest.browser.config.ts.
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
