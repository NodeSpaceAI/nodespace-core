/**
 * Simplified test setup for NodeSpace
 * Provides basic testing utilities without over-complexity
 */
import { vi, beforeEach, afterEach } from 'vitest';
import '@testing-library/jest-dom';
import { PersistenceCoordinator } from '$lib/services/persistence-coordinator.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store';

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

// Mock window.__TAURI__ as undefined to force HTTP adapter usage in tests
// This ensures tests use the HTTP dev server instead of Tauri IPC
if (typeof window !== 'undefined') {
  // Ensure __TAURI__ is undefined for HTTP adapter detection
  Object.defineProperty(window, '__TAURI__', {
    value: undefined,
    writable: false,
    configurable: true
  });
}

// CRITICAL: Plugin registry initialization MUST happen in setup.ts, not global-setup.ts
// This ensures plugins are registered in the same module context (Happy-DOM browser env)
// as the Svelte components that will use them. Global setup runs in Node context,
// which creates a separate module graph and duplicate registry instances.
import { pluginRegistry } from '$lib/plugins/index';
import { registerCorePlugins } from '$lib/plugins/core-plugins';

// Register core plugins once at module load time
// Idempotent guard: safe to call multiple times
try {
  if (!pluginRegistry.hasPlugin('text')) {
    registerCorePlugins(pluginRegistry);
    console.log('✅ Core plugins registered in test setup (browser context)');
  } else {
    console.debug('Core plugins already registered, skipping');
  }
} catch (error) {
  console.error('Failed to register core plugins in test setup:', error);
  throw error; // Fail fast - tests can't run without plugins
}

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

// Mock Tauri environment detection
// Tests run in browser mode (without Tauri), so __TAURI__ should not be defined
// This prevents TauriNodeService from trying to invoke Rust commands
if (typeof window !== 'undefined') {
  // Ensure __TAURI__ is undefined so isTauriEnvironment() returns false
  delete (window as unknown as Record<string, unknown>).__TAURI__;
}

// PersistenceCoordinator and SharedNodeStore cleanup for each test
// NOTE: Coordinator runs in test mode (errors caught gracefully)
// TODO (#248): Future work - Replace test mode with proper vi.mock() of tauriNodeService
beforeEach(() => {
  // Reset SharedNodeStore to clear all nodes between tests
  sharedNodeStore.__resetForTesting();

  // Reset PersistenceCoordinator
  const coordinator = PersistenceCoordinator.getInstance();
  coordinator.resetTestState();
});

afterEach(async () => {
  const coordinator = PersistenceCoordinator.getInstance();
  // Reset and wait for cancellation cleanup
  await coordinator.reset();
});
