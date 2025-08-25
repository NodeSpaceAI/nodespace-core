/**
 * Early setup for Svelte 5 mocks
 * This must run before any Svelte components are imported
 */

// Mock Svelte 5 runes for testing compatibility
// $state mock - creates a simple reactive-like object for tests

function createMockState<T>(initialValue: T): T {
  // For primitive values, just return the value directly
  if (typeof initialValue !== 'object' || initialValue === null) {
    return initialValue;
  }

  // For objects (like Map, Array), return the original object
  // This provides basic compatibility without complex reactivity
  return initialValue;
}

// Mock $state function - available globally
(globalThis as unknown as { $state?: <T>(value: T) => T }).$state = function <T>(
  initialValue: T
): T {
  return createMockState(initialValue);
};
