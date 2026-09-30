/// <reference types="vitest" />
import { defineConfig } from 'vitest/config';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { nodespaceExtensions } from './vite-plugins/nodespace-extensions.js';
import {
  EXTENSIONS_ENV,
  resolveExtensionsEntry
} from './vite-plugins/nodespace-extensions-entry.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/**
 * Vitest config for e2e tests that spawn a real nodespaced + dev-proxy.
 *
 * Run with: bun run test:e2e
 *
 * Prerequisites:
 *   - nodespaced binary in packages/desktop-app/src-tauri/binaries/ (prebuilt sidecar)
 *   - Or set NODESPACED_BINARY=/path/to/nodespaced
 *
 * Optional env vars:
 *   E2E_DAEMON_TIMEOUT — ms to wait for daemon readiness (default 10000)
 *   E2E_VERBOSE        — set to any value to print daemon/proxy stdout
 */
export default defineConfig({
  plugins: [
    nodespaceExtensions({
      root: __dirname,
      entry: resolveExtensionsEntry(process.env[EXTENSIONS_ENV], __dirname)
    })
  ],

  resolve: {
    alias: {
      $lib: path.resolve(__dirname, 'src/lib'),
      '@nodespace/extension-api': path.resolve(__dirname, 'src/lib/extension-api')
    }
  },

  test: {
    include: ['src/tests/e2e/**/*.e2e.ts'],
    environment: 'node',
    globals: true,

    // Define Svelte 5 rune globals ($state/$derived/$effect) inside each test
    // worker so rune-store modules (`*.svelte.ts`) imported by e2e tests don't
    // throw in the plain `node` environment, which has no Svelte compiler
    // (ADR-049). Runs in the fork, unlike a Vite config-time plugin.
    setupFiles: ['src/tests/setup-svelte-mocks.ts'],

    // E2e tests start real processes — allow longer timeouts
    testTimeout: 30_000,
    hookTimeout: 20_000,

    // Each test file gets its own daemon instance via beforeAll/afterAll,
    // so run files sequentially to avoid port conflicts
    pool: 'forks',
    poolOptions: {
      forks: {
        singleFork: true
      }
    },

    sequence: {
      concurrent: false,
      shuffle: false
    }
  }
});
