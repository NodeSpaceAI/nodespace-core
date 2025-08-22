/**
 * NodeSpace Test Setup - Bun + Happy DOM
 * 
 * Optimized for Bun runtime with happy-dom for fast, reliable DOM testing.
 * No Node.js or jsdom dependencies required.
 */
import { vi } from 'vitest';
import '@testing-library/jest-dom';

// Ensure global object is available for legacy test compatibility
// In Vitest/Bun environment, global should already be available, but provide fallback
if (typeof global === 'undefined') {
  (globalThis as unknown as { global: typeof globalThis }).global = globalThis;
} else {
  // Ensure global and globalThis are properly linked
  if (globalThis.global !== globalThis) {
    Object.setPrototypeOf(globalThis.global, globalThis);
  }
}

// Happy DOM is configured via vitest.config.ts environment: 'happy-dom'
// No manual DOM setup required - Vitest handles it automatically

// Mock MutationObserver for testing
interface MockMutationRecord {
  type: string;
  target: Node;
  addedNodes: Node[];
  removedNodes: Node[];
  previousSibling: Node | null;
  nextSibling: Node | null;
  attributeName: string | null;
  attributeNamespace: string | null;
  oldValue: string | null;
}

interface MockMutationObserver {
  observe: () => void;
  disconnect: () => void;
  takeRecords: () => MockMutationRecord[];
}

(
  globalThis as typeof globalThis & { MutationObserver: new () => MockMutationObserver }
).MutationObserver = vi.fn(() => ({
  observe: vi.fn(),
  disconnect: vi.fn(),
  takeRecords: vi.fn().mockReturnValue([])
}));

// Mock IntersectionObserver for testing
interface MockIntersectionObserver {
  observe: () => void;
  unobserve: () => void;
  disconnect: () => void;
}

(
  globalThis as typeof globalThis & { IntersectionObserver: new () => MockIntersectionObserver }
).IntersectionObserver = vi.fn(() => ({
  observe: vi.fn(),
  unobserve: vi.fn(),
  disconnect: vi.fn()
}));

// Basic global test setup
interface MockResizeObserver {
  observe: () => void;
  unobserve: () => void;
  disconnect: () => void;
}

(
  globalThis as typeof globalThis & { ResizeObserver: new () => MockResizeObserver }
).ResizeObserver = vi.fn(() => ({
  observe: vi.fn(),
  unobserve: vi.fn(),
  disconnect: vi.fn()
}));

// Mock console methods for cleaner test output
const originalConsoleError = console.error;
console.error = (...args) => {
  // Suppress known framework warnings during tests
  const message = args[0];
  if (typeof message === 'string' && message.includes('Warning:')) {
    return;
  }
  originalConsoleError(...args);
};

// Enhanced Event constructor to ensure proper target property
interface MockEventTarget {
  addEventListener: (type: string, listener: () => void) => void;
  removeEventListener: (type: string, listener: () => void) => void;
  dispatchEvent: (event: Event) => boolean;
}

interface TestEventInit {
  bubbles?: boolean;
  cancelable?: boolean;
  composed?: boolean;
  target?: MockEventTarget;
}

const OriginalEvent = globalThis.Event;
globalThis.Event = class extends OriginalEvent {
  constructor(type: string, eventInitDict?: TestEventInit) {
    super(type, eventInitDict);
    // Ensure target is properly set when event is created
    if (!this.target && eventInitDict?.target) {
      Object.defineProperty(this, 'target', {
        value: eventInitDict.target,
        writable: false,
        configurable: true
      });
    }
  }
} as typeof Event;
