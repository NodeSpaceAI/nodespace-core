/**
 * Global setup for Vitest
 * This runs once before all test files
 */

// Mock Svelte 5 runes for testing compatibility
function createMockState<T>(initialValue: T): T {
  // For primitive values, just return the value directly
  if (typeof initialValue !== 'object' || initialValue === null) {
    return initialValue;
  }

  // For objects (like Map, Array), return the original object
  // This provides basic compatibility without complex reactivity
  return initialValue;
}

function createMockEffect(fn: () => void | (() => void)): void {
  // Execute effect immediately in tests
  fn();
}

function createMockDerived<T>(getter: () => T): T {
  // Return the value directly, not an object with a getter
  return getter();
}

// Define Svelte 5 runes globally
const stateFunction = function <T>(initialValue: T): T {
  return createMockState(initialValue);
};

const derivedFunction = {
  by: function <T>(getter: () => T) {
    return createMockDerived(getter);
  }
};

const effectFunction = createMockEffect;

// Global definitions
Object.defineProperty(globalThis, '$state', {
  value: stateFunction,
  writable: true,
  configurable: true
});

Object.defineProperty(globalThis, '$derived', {
  value: derivedFunction,
  writable: true,
  configurable: true
});

Object.defineProperty(globalThis, '$effect', {
  value: effectFunction,
  writable: true,
  configurable: true
});

// Also define on global for Node.js compatibility
if (typeof global !== 'undefined') {
  (global as Record<string, unknown>).$state = stateFunction;
  (global as Record<string, unknown>).$derived = derivedFunction;
  (global as Record<string, unknown>).$effect = effectFunction;
}

// Initialize global plugin registry for all tests
import { pluginRegistry } from '$lib/plugins/pluginRegistry';
import { registerCorePlugins } from '$lib/plugins/corePlugins';

// Register core plugins globally for all tests - only once per test run
if (!pluginRegistry.hasPlugin('text')) {
  registerCorePlugins(pluginRegistry);
}

export default async function setup() {
  // Global setup runs once before all tests
  console.log('Global test setup complete: Svelte 5 runes mocked, plugin registry initialized');
}
