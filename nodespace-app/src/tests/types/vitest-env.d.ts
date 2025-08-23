/**
 * Vitest environment type definitions
 * Provides proper typing for test globals and utilities
 */

/// <reference types="vitest/globals" />
/// <reference types="@testing-library/jest-dom" />
/// <reference types="node" />

declare global {
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
