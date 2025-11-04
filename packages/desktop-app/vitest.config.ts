/// <reference types="vitest" />
/// <reference types="./src/tests/types/globals.d.ts" />
/// <reference types="./src/tests/types/vitest-env.d.ts" />
import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { injectRuneMocks } from './src/tests/vite-plugin-inject-runes';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// Early Svelte 5 rune mocks - must be defined BEFORE any module imports
function createMockState<T>(initialValue: T): T {
  // Return primitive values directly
  if (typeof initialValue !== 'object' || initialValue === null) {
    return initialValue;
  }

  // For objects, only create Proxy if it's a plain object or array
  // Avoid creating proxies for complex objects that might cause issues
  if (
    Array.isArray(initialValue) ||
    (typeof initialValue === 'object' && Object.getPrototypeOf(initialValue) === Object.prototype)
  ) {
    try {
      return new Proxy(initialValue as object, {
        set(target: object, property: string | symbol, value: unknown): boolean {
          (target as Record<string | symbol, unknown>)[property] = value;
          return true;
        },
        get(target: object, property: string | symbol): unknown {
          return (target as Record<string | symbol, unknown>)[property];
        }
      }) as T;
    } catch (proxyError) {
      // If proxy creation fails, return the original object
      // Proxy errors are expected for certain object types
      if (proxyError instanceof Error) {
        // Error handled by returning original value
      }
      return initialValue;
    }
  }

  // For other complex objects (Maps, Sets, DOM objects, etc.), return as-is
  return initialValue;
}

function createMockDerived() {
  return {
    by: function <T>(getter: () => T): T {
      // Execute getter immediately and return result
      try {
        if (typeof getter === 'function') {
          return getter();
        }
        // If getter is not a function, return it as-is (fallback)
        return getter as T;
      } catch (getterError) {
        // Derived getter error in test - return undefined as fallback
        if (getterError instanceof Error) {
          // Error handled by returning undefined
        }
        return undefined as T;
      }
    }
  };
}

function createMockEffect(fn: () => void | (() => void)): void {
  // Execute effect immediately in tests
  try {
    if (typeof fn === 'function') {
      const result = fn();
      // If the effect returns a cleanup function, we could store it but for tests we ignore it
      if (typeof result === 'function') {
        // Cleanup function returned - in real Svelte this would be called on component destroy
        // For tests, we just ignore it
      }
    }
  } catch (effectError) {
    // Ignore effect errors in tests - they often depend on DOM or other runtime features
    if (effectError instanceof Error) {
      // Error handled by ignoring - effects may depend on runtime features not available in tests
    }
  }
}

// Set up mocks immediately at module level
const stateFunction = function <T>(initialValue: T): T {
  return createMockState(initialValue);
};

const derivedFunction = createMockDerived();
const effectFunction = createMockEffect;

// Ensure mocks exist on globalThis before any imports - using type assertion to avoid conflicts
try {
  if (!globalThis.$state) {
    (globalThis as Record<string, unknown>).$state = stateFunction;
  }
  if (!globalThis.$derived) {
    (globalThis as Record<string, unknown>).$derived = derivedFunction;
  }
  if (!globalThis.$effect) {
    (globalThis as Record<string, unknown>).$effect = effectFunction;
  }
} catch (setupError) {
  // Setup errors are non-fatal - mocks will be handled by plugin if this fails
  if (setupError instanceof Error) {
    // Error handled - plugin will provide fallback
  }
}

// Also ensure they're available on the global object for Node.js environment
if (typeof globalThis !== 'undefined' && 'global' in globalThis) {
  try {
    const globalObj = (globalThis as { global: Record<string, unknown> }).global;
    if (!globalObj.$state) globalObj.$state = stateFunction;
    if (!globalObj.$derived) globalObj.$derived = derivedFunction;
    if (!globalObj.$effect) globalObj.$effect = effectFunction;
  } catch (globalSetupError) {
    // Global setup errors are non-fatal - mocks will be handled by plugin if this fails
    if (globalSetupError instanceof Error) {
      // Error handled - plugin will provide fallback
    }
  }
}

export default defineConfig({
  plugins: [
    injectRuneMocks(), // MUST run first - injects globals
    // Use svelte plugin directly in test mode to control preprocessing
    // Use sveltekit plugin in dev/build mode for full SvelteKit features
    process.env.VITEST
      ? svelte({
          hot: false,
          // Skip all preprocessing in test environment to avoid PostCSS/Happy-DOM conflicts
          // Components will be compiled with raw styles (no PostCSS)
          preprocess: [],
          compilerOptions: {
            // Generate client-side code (not SSR) to enable $effect and other client-only runes
            generate: 'client'
          }
        })
      : sveltekit()
  ],

  // Configure path aliases for test environment (needed when using svelte plugin instead of sveltekit)
  resolve: {
    alias: {
      $lib: path.resolve(__dirname, 'src/lib'),
      $app: path.resolve(__dirname, 'node_modules/@sveltejs/kit/src/runtime/app')
    },
    // Force browser conditions to load client-side Svelte runtime
    conditions: ['browser', 'import']
  },

  // Disable CSS preprocessing in tests to prevent PostCSS/Happy-DOM incompatibility
  css: {
    postcss: false
  },

  test: {
    include: ['src/tests/**/*.{test,spec}.{js,ts}'],
    exclude: ['src/tests/browser/**'], // Browser tests run separately with test:browser
    environment: 'happy-dom', // Fast, modern DOM for Bun compatibility
    globals: true,
    setupFiles: ['src/tests/setup-svelte-mocks.ts', 'src/tests/setup.ts'],

    // Global setup runs once before all test files
    globalSetup: 'src/tests/global-setup.ts',
    environmentOptions: {
      node: {
        // Ensure global object is available
        global: true
      }
    },

    // Simple coverage configuration
    coverage: {
      provider: 'v8', // Use V8 coverage provider (fast and accurate)
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
    hookTimeout: 5000,

    // CRITICAL: Run tests sequentially to prevent SharedNodeStore interference (Issue #228, #255)
    // SharedNodeStore is a singleton, so parallel tests within a file share state.
    // Even though singleFork: true runs test FILES sequentially, individual tests
    // within a file can run concurrently, causing state corruption.
    sequence: {
      concurrent: false, // Run all tests sequentially, never in parallel
      shuffle: false // Don't randomize test order
    },

    // Run integration tests sequentially to prevent database race conditions (Issue #255)
    // Each test swaps the dev-server's database path, so parallel execution causes
    // "no such table" errors when operations read the wrong database
    pool: 'forks',
    poolOptions: {
      forks: {
        singleFork: true // Run all tests in a single process sequentially
      }
    }
  }
});
