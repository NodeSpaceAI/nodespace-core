/**
 * Vite plugin to inject Svelte 5 rune mocks at the earliest possible stage
 */
import type { Plugin } from 'vite';

export function injectRuneMocks(): Plugin {
  return {
    name: 'inject-rune-mocks',
    enforce: 'pre', // Run before other plugins

    config() {
      // Inject globals at config time - earliest possible
      const stateFunction = function <T>(initialValue: T): T {
        if (typeof initialValue !== 'object' || initialValue === null) {
          return initialValue;
        }
        return initialValue;
      };

      const derivedFunction = {
        by: function <T>(getter: () => T) {
          try {
            return getter();
          } catch {
            return undefined as T;
          }
        }
      };

      const effectFunction = (fn: () => void | (() => void)): void => {
        try {
          fn();
        } catch {
          // Silently ignore errors in test effect functions
        }
      };

      // Inject into globalThis immediately
      (globalThis as unknown as Record<string, unknown>).$state = stateFunction;
      (globalThis as unknown as Record<string, unknown>).$derived = derivedFunction;
      (globalThis as unknown as Record<string, unknown>).$effect = effectFunction;

      if (typeof global !== 'undefined') {
        (global as unknown as Record<string, unknown>).$state = stateFunction;
        (global as unknown as Record<string, unknown>).$derived = derivedFunction;
        (global as unknown as Record<string, unknown>).$effect = effectFunction;
      }

      console.log('✅ Svelte rune mocks injected via Vite plugin');
    }
  };
}
