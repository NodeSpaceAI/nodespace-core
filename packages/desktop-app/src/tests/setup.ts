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

// Mock window.getSelection for testing
// This eliminates "Not implemented: window.getSelection" warnings from Happy-DOM
if (typeof window !== 'undefined' && !window.getSelection) {
  class MockSelection {
    anchorNode: Node | null = null;
    anchorOffset: number = 0;
    focusNode: Node | null = null;
    focusOffset: number = 0;
    isCollapsed: boolean = true;
    rangeCount: number = 0;
    type: string = 'None';
    private ranges: Range[] = [];

    addRange(range: Range): void {
      this.ranges.push(range);
      this.rangeCount = this.ranges.length;
      this.anchorNode = range.startContainer;
      this.anchorOffset = range.startOffset;
      this.focusNode = range.endContainer;
      this.focusOffset = range.endOffset;
      this.isCollapsed = range.collapsed;
      this.type = range.collapsed ? 'Caret' : 'Range';
    }

    getRangeAt(index: number): Range {
      if (index < 0 || index >= this.ranges.length) {
        throw new Error('IndexSizeError: Index out of range');
      }
      return this.ranges[index];
    }

    removeAllRanges(): void {
      this.ranges = [];
      this.rangeCount = 0;
      this.anchorNode = null;
      this.anchorOffset = 0;
      this.focusNode = null;
      this.focusOffset = 0;
      this.isCollapsed = true;
      this.type = 'None';
    }

    removeRange(range: Range): void {
      const index = this.ranges.indexOf(range);
      if (index !== -1) {
        this.ranges.splice(index, 1);
        this.rangeCount = this.ranges.length;
        if (this.rangeCount === 0) {
          this.removeAllRanges();
        }
      }
    }

    collapse(node: Node | null, offset?: number): void {
      this.removeAllRanges();
      if (node) {
        const range = document.createRange();
        range.setStart(node, offset || 0);
        range.setEnd(node, offset || 0);
        this.addRange(range);
      }
    }

    collapseToStart(): void {
      if (this.rangeCount > 0) {
        const range = this.ranges[0];
        this.collapse(range.startContainer, range.startOffset);
      }
    }

    collapseToEnd(): void {
      if (this.rangeCount > 0) {
        const range = this.ranges[0];
        this.collapse(range.endContainer, range.endOffset);
      }
    }

    extend(node: Node, offset?: number): void {
      if (this.rangeCount > 0) {
        const range = this.ranges[0];
        range.setEnd(node, offset || 0);
        this.focusNode = node;
        this.focusOffset = offset || 0;
        this.isCollapsed = range.collapsed;
        this.type = range.collapsed ? 'Caret' : 'Range';
      }
    }

    selectAllChildren(node: Node): void {
      this.removeAllRanges();
      const range = document.createRange();
      range.selectNodeContents(node);
      this.addRange(range);
    }

    setBaseAndExtent(
      anchorNode: Node,
      anchorOffset: number,
      focusNode: Node,
      focusOffset: number
    ): void {
      this.removeAllRanges();
      const range = document.createRange();
      range.setStart(anchorNode, anchorOffset);
      range.setEnd(focusNode, focusOffset);
      this.addRange(range);
    }

    toString(): string {
      if (this.rangeCount === 0) {
        return '';
      }
      return this.ranges.map((range) => range.toString()).join('');
    }
  }

  const mockSelection = new MockSelection();
  window.getSelection = vi.fn(() => mockSelection as unknown as Selection);
}

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
