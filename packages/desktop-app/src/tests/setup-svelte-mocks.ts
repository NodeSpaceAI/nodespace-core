/**
 * Early setup for Svelte 5 mocks
 * This must run before any Svelte components are imported
 */

// Declare global types for Svelte runes
declare global {
  interface Window {
    $state: <T>(initialValue: T) => T;
    $derived: { by: <T>(getter: () => T) => T };
    $effect: (fn: () => void | (() => void)) => void;
  }
}

// Mock Svelte 5 runes for testing compatibility

function createMockState<T>(initialValue: T): T {
  // For primitive values, just return the value directly
  if (typeof initialValue !== 'object' || initialValue === null) {
    return initialValue;
  }

  // For objects (like Map, Array), return the original object safely
  // This provides basic compatibility without complex reactivity
  // Only attempt proxy creation for plain objects and arrays
  if (
    Array.isArray(initialValue) ||
    (typeof initialValue === 'object' && Object.getPrototypeOf(initialValue) === Object.prototype)
  ) {
    try {
      // Simple proxy that just forwards operations - only for plain objects
      return new Proxy(initialValue as object, {
        set(target: object, property: string | symbol, value: unknown): boolean {
          (target as Record<string | symbol, unknown>)[property] = value;
          return true;
        },
        get(target: object, property: string | symbol): unknown {
          return (target as Record<string | symbol, unknown>)[property];
        }
      }) as T;
    } catch {
      // If proxy creation fails, return the original object
      return initialValue;
    }
  }

  // For complex objects (Maps, Sets, etc.), return as-is
  return initialValue;
}

// Mock effect function (used in some reactive code)
function createMockEffect(fn: () => void | (() => void)): void {
  // Execute effect immediately in tests and ignore cleanup
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

// Mock functions with TypeScript compatibility
const stateFunction = function <T>(initialValue: T): T {
  return createMockState(initialValue);
};

const derivedFunction = {
  by: function <T>(getter: () => T) {
    // Return the value directly, not an object with a getter
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

const effectFunction = createMockEffect;

// CRITICAL: Set up mocks immediately before any imports can happen
// Define on globalThis first - using type assertion to avoid conflicts with Svelte's full runtime types
try {
  if (!globalThis.$state) (globalThis as Record<string, unknown>).$state = stateFunction;
  if (!globalThis.$derived) (globalThis as Record<string, unknown>).$derived = derivedFunction;
  if (!globalThis.$effect) (globalThis as Record<string, unknown>).$effect = effectFunction;
} catch (error) {
  console.warn('Warning setting up globalThis Svelte rune mocks:', error);
}

// Also define on global for Node.js compatibility (Vitest environment)
if (typeof global !== 'undefined') {
  try {
    if (!global.$state) (global as Record<string, unknown>).$state = stateFunction;
    if (!global.$derived) (global as Record<string, unknown>).$derived = derivedFunction;
    if (!global.$effect) (global as Record<string, unknown>).$effect = effectFunction;
  } catch (error) {
    console.warn('Warning setting up Node.js global Svelte rune mocks:', error);
  }
}

// Ensure the mocks are available in window context as well (if DOM environment)
if (typeof window !== 'undefined') {
  try {
    if (!window.$state) (window as unknown as Record<string, unknown>).$state = stateFunction;
    if (!window.$derived) (window as unknown as Record<string, unknown>).$derived = derivedFunction;
    if (!window.$effect) (window as unknown as Record<string, unknown>).$effect = effectFunction;
  } catch (error) {
    console.warn('Warning setting up window Svelte rune mocks:', error);
  }
}

// Initialize global plugin registry for all tests
import { pluginRegistry } from '$lib/plugins/pluginRegistry';
import { registerCorePlugins } from '$lib/plugins/corePlugins';

// Register core plugins globally for all tests - only once per test run
if (!pluginRegistry.hasPlugin('text')) {
  registerCorePlugins(pluginRegistry);
}
