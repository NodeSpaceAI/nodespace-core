/**
 * Simplified test setup for NodeSpace
 * Provides basic testing utilities without over-complexity
 */
import { vi } from 'vitest';
import '@testing-library/jest-dom';

// Ensure global object is available for legacy test compatibility
// In Vitest/Node.js environment, global should already be available, but provide fallback
if (
  typeof (globalThis as typeof globalThis & { global?: typeof globalThis }).global === 'undefined'
) {
  (globalThis as typeof globalThis & { global: typeof globalThis }).global = globalThis;
} else {
  const globalRef = (globalThis as typeof globalThis & { global: typeof globalThis }).global;
  // Ensure global and globalThis are properly linked
  if (globalRef !== globalThis) {
    Object.setPrototypeOf(globalRef, globalThis);
  }
}

// DOM globals are provided by happy-dom environment
// No need to set up additional DOM globals - happy-dom handles this

// Plugin registry initialization moved to globalSetup.ts

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
const mockIntersectionObserver = vi.fn(() => ({
  observe: vi.fn(),
  unobserve: vi.fn(),
  disconnect: vi.fn(),
  root: null,
  rootMargin: '',
  thresholds: [],
  takeRecords: vi.fn().mockReturnValue([])
}));

// Type assertion for IntersectionObserver mock
(
  globalThis as typeof globalThis & { IntersectionObserver: typeof mockIntersectionObserver }
).IntersectionObserver = mockIntersectionObserver;

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
    // Extract standard EventInit properties for parent constructor
    const { target, ...standardEventInit } = eventInitDict || {};
    super(type, standardEventInit);
    // Ensure target is properly set when event is created
    if (!this.target && target) {
      Object.defineProperty(this, 'target', {
        value: target,
        writable: false,
        configurable: true
      });
    }
  }
} as typeof Event;
