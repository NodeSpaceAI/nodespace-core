/// <reference types="vitest" />
/// <reference types="./src/tests/types/globals.d.ts" />
/// <reference types="./src/tests/types/vitest-env.d.ts" />
import { defineConfig } from 'vitest/config';
import { sveltekit } from '@sveltejs/kit/vite';

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
    } catch (error) {
      // If proxy creation fails, return the original object
      console.warn('Failed to create proxy for state mock, returning original value:', error);
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
      } catch (error) {
        console.warn('Derived getter error in test (returning undefined):', error);
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
  } catch (error) {
    // Ignore effect errors in tests - they often depend on DOM or other runtime features
    console.warn('Effect error in test (ignored):', error);
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
} catch (error) {
  // Log any issues but don't fail the configuration
  console.warn('Warning setting up Svelte rune mocks:', error);
}

// Also ensure they're available on the global object for Node.js environment
if (typeof globalThis !== 'undefined' && 'global' in globalThis) {
  try {
    const globalObj = globalThis.global as Record<string, unknown>;
    if (!globalObj.$state) globalObj.$state = stateFunction;
    if (!globalObj.$derived) globalObj.$derived = derivedFunction;
    if (!globalObj.$effect) globalObj.$effect = effectFunction;
  } catch (error) {
    console.warn('Warning setting up Node.js global Svelte rune mocks:', error);
  }
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
    hookTimeout: 5000
  }
});
