/// <reference types="vitest" />
/// <reference types="./src/tests/types/globals.d.ts" />
/// <reference types="./src/tests/types/vitest-env.d.ts" />
import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';

// Early Svelte 5 rune mocks - must be defined BEFORE any module imports
function createMockState<T>(initialValue: T): T {
  // For objects and arrays, create a Proxy to enable reactivity-like behavior
  if (typeof initialValue === 'object' && initialValue !== null) {
    return new Proxy(initialValue, {
      set(target, property, value) {
        (target as any)[property] = value;
        return true;
      },
      get(target, property) {
        return (target as any)[property];
      }
    });
  }
  return initialValue;
}

function createMockDerived() {
  return {
    by: function <T>(getter: () => T): T {
      // Execute getter immediately and return result
      return getter();
    }
  };
}

function createMockEffect(fn: () => void | (() => void)): void {
  // Execute effect immediately in tests
  try {
    fn();
  } catch (error) {
    // Ignore effect errors in tests
    console.warn('Effect error in test:', error);
  }
}

// Set up mocks immediately at module level
const stateFunction = function <T>(initialValue: T): T {
  return createMockState(initialValue);
};

const derivedFunction = createMockDerived();
const effectFunction = createMockEffect;

// Ensure mocks exist on globalThis before any imports
try {
  (globalThis as any).$state = stateFunction;
  (globalThis as any).$derived = derivedFunction;
  (globalThis as any).$effect = effectFunction;
} catch {
  // If properties already exist, that's fine
}

// Also ensure they're available on the global object for Node.js environment
if (typeof global !== 'undefined') {
  (global as any).$state = stateFunction;
  (global as any).$derived = derivedFunction;
  (global as any).$effect = effectFunction;
}

export default defineConfig({
  plugins: [sveltekit()],

  test: {
    include: ['src/tests/**/*.{test,spec}.{js,ts}'],
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
