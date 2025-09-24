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

// Type-safe interface for Svelte runes on global objects
export interface SvelteRunes {
  $state: <T>(initialValue: T) => T;
  $derived: {
    by: <T>(getter: () => T) => T;
  };
  $effect: (fn: () => void | (() => void)) => void;
}

// Extend existing global interfaces with type safety
type ExtendedGlobal = typeof globalThis &
  SvelteRunes & {
    document?: TestDocument;
    window?: TestWindow;
    [key: string]: unknown;
  };

declare global {
  /**
   * Node.js global object (available in test environment)
   * This allows tests to use both `global` and `globalThis` interchangeably
   */
  var global: ExtendedGlobal;

  /**
   * Mock Svelte 5 $state rune for testing
   * Simplified mock version without full Svelte runtime properties
   */
  var $state: <T>(initialValue: T) => T;

  /**
   * Mock Svelte 5 $derived rune for testing
   * Simplified mock version without full Svelte runtime properties
   */
  var $derived: {
    by: <T>(getter: () => T) => T;
  };

  /**
   * Mock Svelte 5 $effect rune for testing
   * Simplified mock version without full Svelte runtime properties
   */
  var $effect: (fn: () => void | (() => void)) => void;

  /**
   * Additional test-specific global extensions with Svelte runes
   */
  namespace globalThis {
    // Extend globalThis with Svelte runes
    var $state: <T>(initialValue: T) => T;
    var $derived: {
      by: <T>(getter: () => T) => T;
    };
    var $effect: (fn: () => void | (() => void)) => void;

    interface Window extends SvelteRunes {
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
