/**
 * Global setup for Vitest
 * This runs once before all test files
 *
 * CRITICAL: Svelte runes MUST be defined at module parse time, not in the async function
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
  try {
    fn();
  } catch (error) {
    console.warn('Effect error in test (ignored):', error);
  }
}

function createMockDerived<T>(getter: () => T): T {
  // Return the value directly, not an object with a getter
  try {
    return getter();
  } catch (error) {
    console.warn('Derived getter error in test (returning undefined):', error);
    return undefined as T;
  }
}

// Define Svelte 5 runes globally - IMMEDIATELY at module parse time
const stateFunction = function <T>(initialValue: T): T {
  return createMockState(initialValue);
};

const derivedFunction = {
  by: function <T>(getter: () => T) {
    return createMockDerived(getter);
  }
};

const effectFunction = createMockEffect;

// Set globals IMMEDIATELY - not inside the async function
(globalThis as unknown as Record<string, unknown>).$state = stateFunction;
(globalThis as unknown as Record<string, unknown>).$derived = derivedFunction;
(globalThis as unknown as Record<string, unknown>).$effect = effectFunction;

// Also define on global for Node.js compatibility
if (typeof global !== 'undefined') {
  (global as unknown as Record<string, unknown>).$state = stateFunction;
  (global as unknown as Record<string, unknown>).$derived = derivedFunction;
  (global as unknown as Record<string, unknown>).$effect = effectFunction;
}

console.log('✅ Svelte 5 runes mocked at module parse time');

export default async function setup() {
  // CRITICAL: Validate test environment FIRST
  // Ensure we're running with vitest, not bun test
  if (!process.env.VITEST) {
    console.error(`
╔═══════════════════════════════════════════════════════════════╗
║                    ⚠️  INCORRECT TEST COMMAND ⚠️               ║
╟───────────────────────────────────────────────────────────────╢
║                                                               ║
║  DO NOT use: bun test                                         ║
║                                                               ║
║  This command doesn't support Happy-DOM environment           ║
║  configuration and will cause tests to fail!                  ║
║                                                               ║
║  ✅ CORRECT COMMANDS:                                         ║
║     • bunx vitest run                                         ║
║     • bun run test                                            ║
║                                                               ║
║  Why? Vitest is configured with Happy-DOM in vitest.config.ts ║
║  Bun's native test runner doesn't read this configuration.    ║
║                                                               ║
╚═══════════════════════════════════════════════════════════════╝
`);
    throw new Error(
      "Invalid test environment: Use 'bunx vitest run' or 'bun run test' instead of 'bun test'"
    );
  }

  // Import plugins AFTER runes are available
  const { pluginRegistry } = await import('$lib/plugins/plugin-registry');
  const { registerCorePlugins } = await import('$lib/plugins/core-plugins');

  // Register core plugins globally for all tests - only once per test run
  if (!pluginRegistry.hasPlugin('text')) {
    registerCorePlugins(pluginRegistry);
  }

  console.log('✅ Global test setup complete: Plugin registry initialized');
}
