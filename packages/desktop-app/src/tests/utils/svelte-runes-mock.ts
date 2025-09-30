/**
 * Svelte 5 Runes Mock for Testing
 *
 * Provides simplified reactive behavior for $state, $derived, and $effect
 * that works in test environments without requiring the full Svelte runtime.
 */

// Track all reactive computations
const derivedComputations: Array<() => void> = [];

/**
 * Mock $state - Creates reactive state that triggers derived updates
 */
export function createStateMock<T>(initialValue: T): T {
  // For primitives, just return the value (reactivity handled at assignment level)
  if (typeof initialValue !== 'object' || initialValue === null) {
    return initialValue;
  }

  // For Sets, wrap the Set methods to trigger updates
  if (initialValue instanceof Set) {
    const originalAdd = initialValue.add.bind(initialValue);
    const originalDelete = initialValue.delete.bind(initialValue);
    const originalClear = initialValue.clear.bind(initialValue);

    (initialValue as Set<unknown>).add = function (value: unknown) {
      const result = originalAdd(value);
      // Trigger all derived computations
      for (const compute of derivedComputations) {
        compute();
      }
      return result;
    } as typeof initialValue.add;

    (initialValue as Set<unknown>).delete = function (value: unknown) {
      const result = originalDelete(value);
      // Trigger all derived computations
      for (const compute of derivedComputations) {
        compute();
      }
      return result;
    } as typeof initialValue.delete;

    (initialValue as Set<unknown>).clear = function () {
      originalClear();
      // Trigger all derived computations
      for (const compute of derivedComputations) {
        compute();
      }
    } as typeof initialValue.clear;

    return initialValue;
  }

  // For objects/arrays, create a proxy that triggers updates on mutations
  const handler: ProxyHandler<object> = {
    set(target: object, prop: string | symbol, value: unknown): boolean {
      // Update the property
      Reflect.set(target, prop, value);

      // Trigger all derived computations
      for (const compute of derivedComputations) {
        compute();
      }

      return true;
    },

    deleteProperty(target: object, prop: string | symbol): boolean {
      const result = Reflect.deleteProperty(target, prop);

      // Trigger all derived computations
      for (const compute of derivedComputations) {
        compute();
      }

      return result;
    }
  };

  return new Proxy(initialValue as object, handler) as T;
}

/**
 * Mock $derived.by - Creates computed values that re-run when dependencies change
 */
export function createDerivedMock<T>(getter: () => T): T {
  let cachedValue: T = getter();

  // Create a recompute function
  const recompute = () => {
    cachedValue = getter();
  };

  // Register this computation
  derivedComputations.push(recompute);

  // For arrays, create a Proxy that forwards all array operations
  if (Array.isArray(cachedValue)) {
    return new Proxy([] as object, {
      get(_target, prop: string | symbol) {
        // Always recompute to get latest value
        recompute();

        // Handle special properties
        if (prop === 'length') {
          return (cachedValue as unknown[]).length;
        }

        // For array methods, bind them to the cached value
        const value = Reflect.get(cachedValue as object, prop);
        if (typeof value === 'function') {
          return value.bind(cachedValue);
        }

        return value;
      },

      has(_target, prop: string | symbol) {
        recompute();
        return Reflect.has(cachedValue as object, prop);
      },

      ownKeys(_target) {
        recompute();
        return Reflect.ownKeys(cachedValue as object);
      },

      getOwnPropertyDescriptor(_target, prop: string | symbol) {
        recompute();
        return Reflect.getOwnPropertyDescriptor(cachedValue as object, prop);
      }
    }) as T;
  }

  // For other objects, create a simple proxy
  return new Proxy({} as object, {
    get(_target, prop: string | symbol) {
      recompute();
      return Reflect.get(cachedValue as object, prop);
    },

    has(_target, prop: string | symbol) {
      recompute();
      return Reflect.has(cachedValue as object, prop);
    },

    ownKeys(_target) {
      recompute();
      return Reflect.ownKeys(cachedValue as object);
    },

    getOwnPropertyDescriptor(_target, prop: string | symbol) {
      recompute();
      return Reflect.getOwnPropertyDescriptor(cachedValue as object, prop);
    }
  }) as T;
}

/**
 * Mock $effect - Runs side effects
 */
export function createEffectMock(fn: () => void | (() => void)): void {
  fn();
}

/**
 * Clear all registered derived computations
 * Call this in test cleanup (beforeEach)
 */
export function clearDerivedComputations(): void {
  derivedComputations.length = 0;
}
