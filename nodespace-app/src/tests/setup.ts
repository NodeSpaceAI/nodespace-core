/**
 * Simplified test setup for NodeSpace
 * Provides basic testing utilities without over-complexity
 */
import { vi } from 'vitest';
import '@testing-library/jest-dom';

// Ensure global object is available for legacy test compatibility
// In Vitest/Node.js environment, global should already be available, but provide fallback
if (typeof global === 'undefined') {
  (globalThis as unknown as { global: typeof globalThis }).global = globalThis;
} else {
  // Ensure global and globalThis are properly linked
  if (globalThis.global !== globalThis) {
    Object.setPrototypeOf(globalThis.global, globalThis);
  }
}

// Ensure DOM globals are available
import { JSDOM } from 'jsdom';

const dom = new JSDOM('<!DOCTYPE html><html><body></body></html>', {
  url: 'http://localhost:3000',
  pretendToBeVisual: true,
  resources: 'usable'
});

// Set up global DOM
Object.defineProperty(globalThis, 'window', {
  value: dom.window,
  writable: true
});

Object.defineProperty(globalThis, 'document', {
  value: dom.window.document,
  writable: true
});

Object.defineProperty(globalThis, 'navigator', {
  value: dom.window.navigator,
  writable: true
});

Object.defineProperty(globalThis, 'HTMLElement', {
  value: dom.window.HTMLElement,
  writable: true
});

Object.defineProperty(globalThis, 'Element', {
  value: dom.window.Element,
  writable: true
});

Object.defineProperty(globalThis, 'Node', {
  value: dom.window.Node,
  writable: true
});

Object.defineProperty(globalThis, 'NodeFilter', {
  value: dom.window.NodeFilter,
  writable: true
});

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
