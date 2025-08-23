/**
 * Vitest environment type definitions
 * Provides proper typing for test globals and utilities
 */

/// <reference types="vitest/globals" />
/// <reference types="@testing-library/jest-dom" />
/// <reference types="node" />

import type { TestingLibraryMatchers } from '@testing-library/jest-dom/matchers';

declare global {
  namespace Vi {
    interface JestAssertion<T = unknown> extends TestingLibraryMatchers<T, void> {
      // Vitest assertions interface - add specific method signatures as needed
      toBe: (expected: T) => void;
      toEqual: (expected: T) => void;
      toContain: (expected: unknown) => void;
    }
  }

  /**
   * Ensure the global object is available in test environment
   * In Node.js/Vitest, this should be equivalent to globalThis
   */
  var global: typeof globalThis;

  /**
   * Node.js process object available in test environment
   */
  var process: {
    memoryUsage?: () => {
      heapUsed: number;
      heapTotal: number;
      external: number;
      arrayBuffers: number;
    };
    [key: string]: unknown;
  };
}

export {};
