/// <reference types="vitest" />
/// <reference types="./src/tests/types/globals.d.ts" />
/// <reference types="./src/tests/types/vitest-env.d.ts" />
import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';

// Early Svelte 5 rune mocks - must be defined BEFORE any module imports
function createMockState<T>(initialValue: T): T {
  if (typeof initialValue !== 'object' || initialValue === null) {
    return initialValue;
  }
  return initialValue;
}

function createMockDerived<T>(getter: () => T): { get value(): T } {
  return {
    get value() {
      return getter();
    }
  };
}

function createMockEffect(fn: () => void | (() => void)): void {
  // Execute effect immediately in tests
  fn();
}

// Set up mocks immediately at module level
const stateFunction = function <T>(initialValue: T): T {
  return createMockState(initialValue);
};

const derivedFunction = {
  by: function <T>(getter: () => T) {
    return createMockDerived(getter);
  }
};

const effectFunction = createMockEffect;

// Ensure mocks exist on globalThis before any imports
globalThis.$state = stateFunction as unknown;
globalThis.$derived = derivedFunction as unknown;
globalThis.$effect = effectFunction as unknown;

export default defineConfig({
  plugins: [sveltekit()],

  test: {
    include: ['src/tests/**/*.{test,spec}.{js,ts}'],
    environment: 'happy-dom', // Fast, modern DOM for Bun compatibility
    globals: true,
    setupFiles: ['src/tests/setup-svelte-mocks.ts', 'src/tests/setup.ts'],

    // Global setup runs once, but variables don't persist to test environment
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
