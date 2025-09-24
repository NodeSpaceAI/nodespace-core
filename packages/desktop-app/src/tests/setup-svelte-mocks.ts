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

  // For objects (like Map, Array), return the original object
  // This provides basic compatibility without complex reactivity
  return initialValue;
}

// Mock effect function (used in some reactive code)
function createMockEffect(fn: () => void | (() => void)): void {
  // Execute effect immediately in tests and ignore cleanup
  // Execute effect immediately in tests
  fn();
  // Could store cleanup functions if needed for teardown
}

// Mock functions with TypeScript compatibility
const stateFunction = function <T>(initialValue: T): T {
  return createMockState(initialValue);
};

const derivedFunction = {
  by: function <T>(getter: () => T) {
    // Return the value directly, not an object with a getter
    return getter();
  }
};

const effectFunction = createMockEffect;

// CRITICAL: Set up mocks immediately before any imports can happen
// Define on globalThis first
(globalThis as any).$state = stateFunction;
(globalThis as any).$derived = derivedFunction;
(globalThis as any).$effect = effectFunction;

// Also define on global for Node.js compatibility (Vitest environment)
if (typeof global !== 'undefined') {
  (global as any).$state = stateFunction;
  (global as any).$derived = derivedFunction;
  (global as any).$effect = effectFunction;
}

// Ensure the mocks are available in window context as well (if DOM environment)
if (typeof window !== 'undefined') {
  (window as any).$state = stateFunction;
  (window as any).$derived = derivedFunction;
  (window as any).$effect = effectFunction;
}

// Initialize global plugin registry for all tests
import { pluginRegistry } from '$lib/plugins/pluginRegistry';
import { registerCorePlugins } from '$lib/plugins/corePlugins';

// Register core plugins globally for all tests - only once per test run
if (!pluginRegistry.hasPlugin('text')) {
  registerCorePlugins(pluginRegistry);
}
