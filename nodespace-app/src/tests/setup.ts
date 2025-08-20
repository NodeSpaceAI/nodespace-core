/**
 * Simplified test setup for NodeSpace
 * Provides basic testing utilities without over-complexity
 */
import { vi } from 'vitest';
import '@testing-library/jest-dom';

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
