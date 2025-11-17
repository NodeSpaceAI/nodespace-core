/**
 * Unified Test Helpers for NodeSpace
 *
 * Central location for all test utilities to eliminate duplication
 * across 15+ test files. Provides consistent, well-tested helpers for:
 *
 * - Node creation and manipulation
 * - Async timing and effects
 * - EventBus testing utilities
 * - Mock event handler creation
 * - Component testing utilities
 *
 * Usage:
 * ```typescript
 * import { createTestNode, waitForEffects, createEventLogger } from '@tests/helpers';
 * ```
 */

import { tick } from 'svelte';
import { vi, type MockInstance } from 'vitest';
import type { Node, NodeUIState } from '$lib/types/node';
import { eventBus } from '$lib/services/event-bus';
import type { NodeSpaceEvent } from '$lib/services/event-types';

// ============================================================================
// Node Creation Utilities
// ============================================================================

/**
 * Create a test node with flexible parameter options
 *
 * Supports both positional parameters (legacy) and options object (preferred).
 *
 * @param idOrOptions - Node ID (string) or options object (Partial<Node>)
 * @param content - Node content (when using positional parameters)
 * @param nodeType - Optional node type (when using positional parameters)
 * @param parentId - Parent node ID to set parentId (when using positional parameters)
 * @param additionalProps - Additional properties to override (when using positional parameters)
 * @returns Complete Node object
 *
 * @example
 * ```typescript
 * // Positional parameters (legacy)
 * const node = createTestNode('my-id', 'My content');
 * const childNode = createTestNode('child', 'Child content', 'text', 'parent-id');
 *
 * // Options object (preferred)
 * const node = createTestNode({ id: 'my-id', content: 'My content' });
 * const childNode = createTestNode({ parentId: 'parent-id', content: 'Child' });
 * ```
 */
export function createTestNode(
  idOrOptions?: string | Partial<Node>,
  content?: string,
  nodeType?: string,
  parentId?: string | null, // Parent node ID - sets parentId for hierarchy
  additionalProps?: Partial<Node>
): Node {
  // Handle object-based call (preferred)
  if (typeof idOrOptions === 'object' || idOrOptions === undefined) {
    const options = idOrOptions || {};
    const id = options.id || `test-node-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    const now = new Date().toISOString();

    return {
      id,
      nodeType: options.nodeType || 'text',
      content: options.content ?? 'Test content',
      beforeSiblingId: options.beforeSiblingId ?? null,
      parentId: options.parentId ?? null,
      createdAt: options.createdAt || now,
      modifiedAt: options.modifiedAt || now,
      version: options.version ?? 1,
      properties: options.properties || {},
      // Always provide mentions as an array, never undefined
      mentions: options.mentions || [],
      embeddingVector: options.embeddingVector
    } as Node & { mentions: string[] }; // Type assertion to guarantee mentions is always present
  }

  // Handle legacy positional parameters
  const id = idOrOptions;
  const now = new Date().toISOString();

  return {
    id,
    nodeType: nodeType || 'text',
    content: content ?? 'Test content',
    beforeSiblingId: null,
    parentId: parentId ?? null, // Use parentId parameter for hierarchy
    createdAt: now,
    modifiedAt: now,
    version: 1,
    properties: {},
    embeddingVector: undefined,
    ...additionalProps,
    // Ensure mentions is always an array (override additionalProps if needed)
    mentions: additionalProps?.mentions || []
  } as Node & { mentions: string[] }; // Type assertion to guarantee mentions is always present
}

/**
 * Create a text node with specific content
 *
 * @param content - Text content for the node
 * @returns Text node
 *
 * @example
 * ```typescript
 * const textNode = createTextNode('This is a text node');
 * ```
 */
export function createTextNode(content: string): Node {
  return createTestNode({ nodeType: 'text', content });
}

/**
 * Create a task node with specific content
 *
 * @param content - Task description
 * @returns Task node
 *
 * @example
 * ```typescript
 * const task = createTaskNode('Complete testing setup');
 * ```
 */
export function createTaskNode(content: string): Node {
  return createTestNode({ nodeType: 'task', content });
}

/**
 * Create a document node with specific content
 *
 * @param content - Document content
 * @returns Document node
 *
 * @example
 * ```typescript
 * const doc = createDocumentNode('Project documentation');
 * ```
 */
export function createDocumentNode(content: string): Node {
  return createTestNode({ nodeType: 'document', content });
}

/**
 * Create a child node with specified parent (legacy - hierarchy via backend)
 *
 * @param _parentId - Parent node ID (ignored - use backend hierarchy queries)
 * @param content - Child node content
 * @param overrides - Additional properties to override
 * @returns Node (hierarchy relationships managed via backend)
 *
 * @example
 * ```typescript
 * const parent = createTestNode({ id: 'parent-1' });
 * const child = createNodeWithParent('parent-1', 'Child content');
 * // Note: Actual parent-child relationship must be established via backend
 * ```
 */
export function createNodeWithParent(
  _parentId: string,
  content: string,
  overrides: Partial<Node> = {}
): Node {
  return createTestNode({
    content,
    ...overrides
  });
}

/**
 * Create a node hierarchy (tree structure) - nodes only
 *
 * Creates flat array of nodes. Hierarchy relationships must be established
 * via backend graph queries after node creation.
 *
 * @param depth - Maximum depth of the tree (1 = root only)
 * @param breadth - Number of children per node
 * @returns Array of nodes (hierarchy via backend)
 *
 * @example
 * ```typescript
 * // Create tree: 1 root with 3 children, each with 2 children (depth=3, breadth=2)
 * const nodes = createTestNodeTree(3, 2);
 * // Returns: 1 + 3 + 6 = 10 nodes (hierarchy via backend)
 * ```
 */
export function createTestNodeTree(depth: number, breadth: number): Node[] {
  const nodes: Node[] = [];
  let idCounter = 0;

  const createLevel = (_parentId: string | null, currentDepth: number) => {
    if (currentDepth > depth) return;

    for (let i = 0; i < breadth; i++) {
      const id = `node-${++idCounter}`;
      const node = createTestNode({
        id,
        content: `Node ${id} (depth ${currentDepth})`
      });
      nodes.push(node);

      // Recursively create children
      createLevel(id, currentDepth + 1);
    }
  };

  // Create root nodes
  createLevel(null, 1);
  return nodes;
}

// ============================================================================
// UI State Utilities
// ============================================================================

/**
 * Create default UI state for testing
 *
 * @param nodeId - Node ID
 * @param overrides - Properties to override
 * @returns NodeUIState object
 *
 * @example
 * ```typescript
 * const uiState = createTestUIState('node-1', { expanded: true, depth: 2 });
 * ```
 */
export function createTestUIState(nodeId: string, overrides?: Partial<NodeUIState>): NodeUIState {
  return {
    nodeId,
    depth: 0,
    expanded: false,
    autoFocus: false,
    inheritHeaderLevel: 0,
    isPlaceholder: false,
    ...overrides
  };
}

// ============================================================================
// Async/Timing Utilities
// ============================================================================

/**
 * Wait for Svelte effects to complete
 *
 * Waits for all pending Svelte 5 effects ($effect) and reactive updates
 * to complete. Essential for testing async component behavior.
 *
 * Replaces all raw `setTimeout` patterns across tests.
 *
 * @param additionalMs - Additional milliseconds to wait after tick
 * @returns Promise that resolves when effects are complete
 *
 * @example
 * ```typescript
 * // After triggering a state change
 * await waitForEffects();
 * // Now DOM is updated and effects have run
 * expect(element).toBeInTheDocument();
 *
 * // Wait longer for animations
 * await waitForEffects(100);
 * ```
 */
export async function waitForEffects(additionalMs: number = 0): Promise<void> {
  // Wait for Svelte's reactive updates
  await tick();

  // If additional wait time is needed (for animations, timers, etc.)
  if (additionalMs > 0) {
    await new Promise((resolve) => setTimeout(resolve, additionalMs));
  }
}

/**
 * Wait for EventBus batch processing
 *
 * EventBus batches certain operations. This helper waits for
 * typical batch durations used in tests.
 *
 * @param batchMs - Batch processing time (default: 100ms)
 * @returns Promise that resolves after batch processing
 *
 * @example
 * ```typescript
 * eventBus.emit({ type: 'node:updated', nodeId: 'test-1' });
 * await waitForEventBusBatch();
 * // Now batched handlers have processed
 * ```
 */
export async function waitForEventBusBatch(batchMs: number = 100): Promise<void> {
  await waitForEffects(batchMs);
}

/**
 * Wait for async operation with custom duration
 *
 * Generic async wait utility for tests that need specific timing.
 *
 * @param ms - Milliseconds to wait
 * @returns Promise that resolves after specified time
 *
 * @example
 * ```typescript
 * await waitForAsync(50);
 * ```
 */
export async function waitForAsync(ms: number): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

// ============================================================================
// EventBus Testing Utilities
// ============================================================================

/**
 * Event logger for collecting and analyzing events
 *
 * Captures all events emitted during tests for verification.
 * Useful for verifying event sequences and debugging event flow.
 */
export interface EventLogger {
  /** All captured events */
  events: NodeSpaceEvent[];
  /** Subscribe to eventBus - returns unsubscribe function */
  subscribe: () => () => void;
  /** Clear all captured events */
  clear: () => void;
  /** Get array of event types in order */
  getEventTypes: () => string[];
  /** Find first event of specific type */
  findEvent: (type: string) => NodeSpaceEvent | undefined;
  /** Find all events of specific type */
  findAllEvents: (type: string) => NodeSpaceEvent[];
  /** Check if event type was emitted */
  hasEvent: (type: string) => boolean;
}

/**
 * Create an event logger that collects all events
 *
 * Useful for verifying event sequences and debugging event flow.
 *
 * @returns EventLogger instance
 *
 * @example
 * ```typescript
 * const logger = createEventLogger();
 * const unsubscribe = logger.subscribe();
 *
 * // ... trigger some operations ...
 *
 * expect(logger.events).toHaveLength(3);
 * expect(logger.hasEvent('node:created')).toBe(true);
 * expect(logger.getEventTypes()).toEqual(['node:created', 'node:updated', 'hierarchy:changed']);
 *
 * unsubscribe();
 * ```
 */
export function createEventLogger(): EventLogger {
  const events: NodeSpaceEvent[] = [];

  return {
    events,
    subscribe: () =>
      eventBus.subscribe('*', (e) => {
        events.push(e);
      }),
    clear: () => {
      events.length = 0;
    },
    getEventTypes: () => events.map((e) => e.type),
    findEvent: (type) => events.find((e) => e.type === type),
    findAllEvents: (type) => events.filter((e) => e.type === type),
    hasEvent: (type) => events.some((e) => e.type === type)
  };
}

/**
 * Event collector for specific event types
 *
 * Subscribes to EventBus and collects events of specific type(s).
 */
export interface EventCollector<T extends NodeSpaceEvent = NodeSpaceEvent> {
  /** Collected events */
  events: T[];
  /** Unsubscribe from eventBus */
  unsubscribe: () => void;
  /** Clear collected events */
  clear: () => void;
  /** Wait for N events to be collected */
  waitForCount: (count: number, timeoutMs?: number) => Promise<T[]>;
}

/**
 * Subscribe and collect specific event types
 *
 * @param eventTypes - Event type(s) to collect (string or array)
 * @returns EventCollector instance
 *
 * @example
 * ```typescript
 * const collector = subscribeAndCollect<NodeCreatedEvent>('node:created');
 *
 * // ... trigger operations ...
 *
 * await collector.waitForCount(2);
 * expect(collector.events).toHaveLength(2);
 *
 * collector.unsubscribe();
 * ```
 */
export function subscribeAndCollect<T extends NodeSpaceEvent = NodeSpaceEvent>(
  eventTypes: string | string[]
): EventCollector<T> {
  const events: T[] = [];
  const types = Array.isArray(eventTypes) ? eventTypes : [eventTypes];

  const unsubscribe = eventBus.subscribe('*', (e) => {
    if (types.includes(e.type)) {
      events.push(e as T);
    }
  });

  return {
    events,
    unsubscribe,
    clear: () => {
      events.length = 0;
    },
    waitForCount: async (count: number, timeoutMs: number = 3000): Promise<T[]> => {
      const startTime = Date.now();
      while (events.length < count && Date.now() - startTime < timeoutMs) {
        await waitForAsync(10);
      }
      if (events.length < count) {
        throw new Error(
          `Timeout waiting for ${count} events. Got ${events.length} after ${timeoutMs}ms`
        );
      }
      return events;
    }
  };
}

/**
 * Wait for specific event to be emitted
 *
 * @param eventType - Event type to wait for
 * @param timeoutMs - Maximum time to wait (default: 1000ms)
 * @returns Promise that resolves with the event
 *
 * @example
 * ```typescript
 * const event = await waitForEvent<NodeCreatedEvent>('node:created', 2000);
 * expect(event.nodeId).toBe('test-node-1');
 * ```
 */
export async function waitForEvent<T extends NodeSpaceEvent>(
  eventType: T['type'],
  timeoutMs: number = 1000
): Promise<T> {
  return eventBus.waitFor(eventType, undefined, timeoutMs);
}

/**
 * Verify events fired in expected order
 *
 * Checks that events appear in the specified sequence, though
 * other events may appear between them.
 *
 * @param events - Array of events to check
 * @param expectedTypes - Expected event types in order
 *
 * @example
 * ```typescript
 * const logger = createEventLogger();
 * // ... operations ...
 * expectEventSequence(logger.events, ['node:created', 'node:updated', 'hierarchy:changed']);
 * ```
 */
export function expectEventSequence(events: NodeSpaceEvent[], expectedTypes: string[]): void {
  const actualTypes = events.map((e) => e.type as string);
  let lastIndex = -1;

  for (const expectedType of expectedTypes) {
    const index = actualTypes.indexOf(expectedType, lastIndex + 1);
    if (index === -1) {
      throw new Error(
        `Expected event "${expectedType}" not found after previous events. ` +
          `Event sequence: ${actualTypes.join(' → ')}`
      );
    }
    lastIndex = index;
  }
}

// ============================================================================
// Mock Event Handler Utilities
// ============================================================================

/**
 * Mock event handlers with type safety
 *
 * Creates typed mock functions for component event handlers.
 * Useful for verifying component events are dispatched correctly.
 *
 * @returns Object with event handler mocks
 *
 * @example
 * ```typescript
 * const handlers = createMockEventHandlers<{
 *   select: { id: string; title: string };
 *   close: void;
 * }>();
 *
 * // Attach to component
 * component.$on('select', handlers.select);
 * component.$on('close', handlers.close);
 *
 * // Verify in tests
 * expect(handlers.select).toHaveBeenCalledWith(
 *   expect.objectContaining({ detail: { id: 'node-1' } })
 * );
 * ```
 */
export function createMockEventHandlers<T extends Record<string, unknown>>(): {
  [K in keyof T]: MockInstance;
} {
  return new Proxy(
    {},
    {
      get(_target, _prop) {
        return vi.fn();
      }
    }
  ) as { [K in keyof T]: MockInstance };
}

/**
 * Create mock NodeManager events
 *
 * Standard mock handlers for NodeManager event interface.
 *
 * @returns Object with typed mock handlers
 *
 * @example
 * ```typescript
 * const events = createMockNodeManagerEvents();
 * const service = createMockReactiveNodeService(events);
 *
 * // ... operations ...
 *
 * expect(events.nodeCreated).toHaveBeenCalledWith('node-1');
 * expect(events.hierarchyChanged).toHaveBeenCalled();
 * ```
 */
export function createMockNodeManagerEvents() {
  return {
    focusRequested: vi.fn(),
    hierarchyChanged: vi.fn(),
    nodeCreated: vi.fn(),
    nodeDeleted: vi.fn()
  };
}

/**
 * Verify mock handler was called
 *
 * @param handler - Mock handler to verify
 * @param times - Expected call count (default: at least once)
 *
 * @example
 * ```typescript
 * expectHandlerCalled(handlers.select, 1);
 * expectHandlerCalled(handlers.close);
 * ```
 */
export function expectHandlerCalled(handler: MockInstance, times?: number): void {
  if (times !== undefined) {
    expect(handler).toHaveBeenCalledTimes(times);
  } else {
    expect(handler).toHaveBeenCalled();
  }
}

// ============================================================================
// Component Testing Utilities
// ============================================================================

/**
 * Create a keyboard event with proper configuration
 *
 * Helper for creating KeyboardEvent instances with correct bubbling
 * and event properties for testing keyboard interactions.
 *
 * @param key - Key name (e.g., 'Enter', 'ArrowDown', 'Escape')
 * @param options - Additional event options
 * @returns Configured KeyboardEvent
 *
 * @example
 * ```typescript
 * const enterEvent = createKeyboardEvent('Enter', { shiftKey: true });
 * element.dispatchEvent(enterEvent);
 * ```
 */
export function createKeyboardEvent(
  key: string,
  options: Partial<Pick<KeyboardEvent, 'shiftKey' | 'ctrlKey' | 'altKey' | 'metaKey'>> = {}
): KeyboardEvent {
  return new KeyboardEvent('keydown', {
    key,
    bubbles: true,
    cancelable: true,
    ...options
  });
}

/**
 * Set mock return value with proper Vitest typing
 *
 * Helper to avoid repetitive type casts when setting mock return values.
 * Vitest's vi.fn() returns a generic Mock type that doesn't include mockReturnValue
 * in the type signature, requiring a type cast at each usage.
 *
 * @param fn - Mock function (created with vi.fn())
 * @param value - Value to return from the mock
 *
 * @example
 * ```typescript
 * const mockFn = vi.fn(() => 5);
 * mockReturnValue(mockFn, 10); // Now returns 10
 * ```
 */
export function mockReturnValue<T>(fn: unknown, value: T): void {
  (fn as ReturnType<typeof vi.fn>).mockReturnValue(value);
}

/**
 * Simulate key press on element
 *
 * @param element - Element to dispatch event on
 * @param key - Key to press
 * @param modifiers - Modifier keys (e.g., ['shift', 'ctrl'])
 *
 * @example
 * ```typescript
 * simulateKeyPress(input, 'Enter');
 * simulateKeyPress(input, 's', ['ctrl']);
 * ```
 */
export function simulateKeyPress(
  element: HTMLElement,
  key: string,
  modifiers: string[] = []
): void {
  const event = createKeyboardEvent(key, {
    shiftKey: modifiers.includes('shift'),
    ctrlKey: modifiers.includes('ctrl') || modifiers.includes('control'),
    altKey: modifiers.includes('alt'),
    metaKey: modifiers.includes('meta') || modifiers.includes('cmd')
  });
  element.dispatchEvent(event);
}

/**
 * Verify event propagation was stopped
 *
 * @param event - Event object to check
 * @returns True if stopPropagation was called
 *
 * @example
 * ```typescript
 * const event = new KeyboardEvent('keydown', { key: 'Escape' });
 * event.stopPropagation = vi.fn();
 * element.dispatchEvent(event);
 * expect(expectEventStopped(event)).toBe(true);
 * ```
 */
export function expectEventStopped(event: Event): boolean {
  type StopPropagationFn = Event['stopPropagation'] & { mock?: { calls: unknown[] } };
  const stopPropagation = event.stopPropagation as StopPropagationFn;

  if (typeof stopPropagation === 'function' && stopPropagation.mock) {
    return stopPropagation.mock.calls.length > 0;
  }

  return false;
}

/**
 * Get ARIA attributes from element
 *
 * Helper for extracting and verifying ARIA attributes in tests.
 *
 * @param element - HTML element to inspect
 * @returns Object with ARIA attributes
 *
 * @example
 * ```typescript
 * const aria = getAriaAttributes(listbox);
 * expect(aria.role).toBe('listbox');
 * expect(aria.selected).toBe('true');
 * ```
 */
export function getAriaAttributes(element: HTMLElement) {
  return {
    role: element.getAttribute('role'),
    label: element.getAttribute('aria-label'),
    selected: element.getAttribute('aria-selected'),
    expanded: element.getAttribute('aria-expanded'),
    hidden: element.getAttribute('aria-hidden'),
    disabled: element.getAttribute('aria-disabled'),
    live: element.getAttribute('aria-live'),
    activedescendant: element.getAttribute('aria-activedescendant')
  };
}
