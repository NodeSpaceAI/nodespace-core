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
