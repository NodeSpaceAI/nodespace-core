/**
 * Simplified test setup for NodeSpace
 * Provides basic testing utilities without over-complexity
 */
import { vi } from 'vitest';
import '@testing-library/jest-dom';

// Basic global test setup
interface MockResizeObserver {
  observe: () => void;
  unobserve: () => void;
  disconnect: () => void;
}

(globalThis as typeof globalThis & { ResizeObserver: new () => MockResizeObserver }).ResizeObserver = vi.fn(() => ({
  observe: vi.fn(),
  unobserve: vi.fn(),
  disconnect: vi.fn(),
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