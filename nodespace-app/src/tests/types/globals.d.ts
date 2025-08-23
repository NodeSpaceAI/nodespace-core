/**
 * Global type definitions for test environment
 * Fixes "global is not defined" errors in tests
 */

/// <reference types="node" />
/// <reference lib="dom" />

export interface TestDocument {
  createElement: (tagName: string) => HTMLElement;
  createTreeWalker?: (
    root: Node,
    whatToShow?: number
  ) => {
    nextNode: () => Node | null;
  };
  querySelectorAll?: (selector: string) => Element[];
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
   * Mock Svelte 5 $state rune for testing
   */
  var $state: <T>(initialValue: T) => T;
}
