/**
 * Global type definitions for test environment
 * Fixes "global is not defined" errors in tests
 */

/// <reference types="node" />
/// <reference lib="dom" />

interface TestDocument {
  createElement: (tagName: string) => unknown;
  createTreeWalker?: (
    root: Node,
    whatToShow?: number
  ) => {
    nextNode: () => Node | null;
  };
  querySelectorAll?: (selector: string) => unknown[];
  [key: string]: unknown;
}

interface TestWindow {
  IntersectionObserver?: unknown;
  MutationObserver?: unknown;
  [key: string]: unknown;
}

declare global {
  /**
   * Node.js global object (available in test environment)
   * This allows tests to use both `global` and `globalThis` interchangeably
   */
  var global: typeof globalThis & {
    document?: TestDocument;
    window?: TestWindow;
    [key: string]: unknown;
  };

  /**
   * Additional test-specific global extensions
   */
  namespace globalThis {
    interface Window {
      IntersectionObserver: new (
        callback: (entries: unknown[]) => void,
        options?: unknown
      ) => {
        observe: (target: Element) => void;
        unobserve: (target: Element) => void;
        disconnect: () => void;
      };
      MutationObserver: new (callback: (mutations: unknown[]) => void) => {
        observe: (target: Node, options?: unknown) => void;
        disconnect: () => void;
        takeRecords: () => unknown[];
      };
      ResizeObserver: new (callback: (entries: unknown[]) => void) => {
        observe: (target: Element) => void;
        unobserve: (target: Element) => void;
        disconnect: () => void;
      };
    }
  }
}

/**
 * Additional test environment type augmentations
 * Enable flexible global properties for testing scenarios
 */
export {};
