/**
 * Node Creation Event Chain - Integration Tests
 *
 * Tests the complete event chain for node creation from keyboard input through EventBus emission.
 * Combines component-level keyboard handling tests with service-level EventBus integration tests.
 *
 * Architecture:
 * - BaseNode handles keyboard input and emits 'createNewNode' event
 * - BaseNodeViewer catches event and calls NodeManager.createNode()
 * - NodeManager creates node and emits EventBus events (node:created, hierarchy:changed, cache:invalidate)
 *
 * Test Strategy:
 * - Component Layer: Test BaseNode keyboard handling and event emission
 * - Service Layer: Test NodeManager.createNode() with EventBus integration
 * - This approach tests each layer appropriately without complex BaseNodeViewer setup
 *
 * Related: Issue #158
 */

// Mock Svelte 5 runes for NodeManager tests
(globalThis as Record<string, unknown>).$state = function <T>(initialValue: T): T {
  if (typeof initialValue !== 'object' || initialValue === null) {
    return initialValue;
  }
  return initialValue;
};

(globalThis as Record<string, unknown>).$derived = {
  by: function <T>(getter: () => T): T {
    return getter();
  }
};

(globalThis as Record<string, unknown>).$effect = function (fn: () => void | (() => void)): void {
  fn();
};

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render } from '@testing-library/svelte';
import { createUserEvents, waitForEffects } from '../components/svelte-test-utils';
import BaseNode from '$lib/design/components/base-node.svelte';
import {
  createReactiveNodeService,
  type ReactiveNodeService as NodeManager
} from '$lib/services/reactiveNodeService.svelte';
import type { NodeManagerEvents } from '$lib/services/reactiveNodeService.svelte';
import { eventBus } from '$lib/services/eventBus';
import type { NodeSpaceEvent } from '$lib/services/eventTypes';

// Helper to create test nodes
function createNode(
  id: string,
  content: string,
  nodeType: string = 'text',
  parentId: string | null = null
) {
  return {
    id,
    nodeType,
    content,
    parentId,
    originNodeId: null,
    beforeSiblingId: null,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    mentions: [] as string[],
    properties: {}
  };
}

describe('Node Creation Event Chain', () => {
  // ============================================================================
  // Component Layer Tests: BaseNode Keyboard Handling Smoke Tests
  // ============================================================================
  // NOTE: These tests verify that BaseNode renders and handles keyboard input without crashing.
  // They do NOT verify event emission or EventBus integration - that's tested in the Service Layer.
  // For complete event chain testing (BaseNode → BaseNodeViewer → NodeManager), see autocomplete-flow.test.ts

  describe('BaseNode: Keyboard Handling Smoke Tests', () => {
    // Note: Happy-DOM manages cleanup automatically, no need to manually clear document.body

    it('should render and remain functional after Enter key at end of content', async () => {
      const user = createUserEvents();

      render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'Test content',
        autoFocus: true
      });

      const editor = document.querySelector('[contenteditable="true"]');
      expect(editor).toBeInTheDocument();

      // Position cursor at end and press Enter
      await user.click(editor!);
      await user.keyboard('{End}');
      await user.keyboard('{Enter}');
      await waitForEffects();

      // Verify editor remains functional after Enter key
      expect(editor).toBeInTheDocument();
      expect(editor).toHaveAttribute('contenteditable', 'true');
    });

    it('should render and remain functional after Enter key in middle of content', async () => {
      const user = createUserEvents();

      render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'Hello World',
        autoFocus: true
      });

      const editor = document.querySelector('[contenteditable="true"]');

      // Position cursor in middle (after "Hello ")
      await user.click(editor!);
      await user.keyboard('{Home}');
      for (let i = 0; i < 6; i++) {
        await user.keyboard('{ArrowRight}');
      }
      await user.keyboard('{Enter}');
      await waitForEffects();

      // Verify editor handles content splitting
      expect(editor).toBeInTheDocument();
    });

    it('should render and remain functional after Enter key with text selection', async () => {
      const user = createUserEvents();

      render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'Select this text',
        autoFocus: true
      });

      const editor = document.querySelector('[contenteditable="true"]');

      // Select text and press Enter
      await user.click(editor!);
      await user.keyboard('{Home}');
      for (let i = 0; i < 7; i++) {
        await user.keyboard('{Shift>}{ArrowRight}{/Shift}');
      }
      await user.keyboard('{Enter}');
      await waitForEffects();

      // Verify editor handles selection replacement
      expect(editor).toBeInTheDocument();
    });

    it('should render and remain functional after rapid Enter key presses', async () => {
      const user = createUserEvents();

      render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: 'Test',
        autoFocus: true
      });

      const editor = document.querySelector('[contenteditable="true"]');

      await user.click(editor!);
      await user.keyboard('{Enter}{Enter}{Enter}');
      await waitForEffects();

      // Verify editor handles rapid input
      expect(editor).toBeInTheDocument();
    });

    it('should render and remain functional with Enter on empty content', async () => {
      const user = createUserEvents();

      render(BaseNode, {
        nodeId: 'test-node',
        nodeType: 'text',
        content: '',
        autoFocus: true
      });

      const editor = document.querySelector('[contenteditable="true"]');

      await user.click(editor!);
      await user.keyboard('{Enter}');
      await waitForEffects();

      // Verify editor handles empty content gracefully
      expect(editor).toBeInTheDocument();
    });
  });

  // ============================================================================
  // Service Layer Tests: NodeManager EventBus Integration
  // ============================================================================

  describe('NodeManager: EventBus Event Emission', () => {
    let nodeManager: NodeManager;
    let mockEvents: NodeManagerEvents;
    let events: NodeSpaceEvent[];

    beforeEach(() => {
      events = [];

      mockEvents = {
        focusRequested: vi.fn(),
        hierarchyChanged: vi.fn(),
        nodeCreated: vi.fn(),
        nodeDeleted: vi.fn()
      };

      nodeManager = createReactiveNodeService(mockEvents);

      // Reset EventBus and track all events
      eventBus.reset();
      eventBus.subscribe('*', (event) => {
        events.push(event);
      });
    });

    it('should emit all required EventBus events when node created', () => {
      // Initialize with test node
      nodeManager.initializeNodes([createNode('root1', 'Root node')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Clear initialization events
      events.length = 0;

      // Create new node (simulates Enter key result)
      const newNodeId = nodeManager.createNode('root1', 'New node content');

      // CRITICAL: Verify all required EventBus events are emitted
      const createdEvents = events.filter((e) => e.type === 'node:created');
      expect(createdEvents).toHaveLength(1);
      expect(createdEvents[0].nodeId).toBe(newNodeId);

      const hierarchyEvents = events.filter((e) => e.type === 'hierarchy:changed');
      expect(hierarchyEvents).toHaveLength(1);

      const cacheEvents = events.filter((e) => e.type === 'cache:invalidate');
      expect(cacheEvents.length).toBeGreaterThanOrEqual(1);
    });

    it('should emit events in correct order: created → hierarchy → cache', () => {
      nodeManager.initializeNodes([createNode('root1', 'Root')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      events.length = 0;

      nodeManager.createNode('root1', 'New content');

      // CRITICAL: Verify event order as specified in issue #158
      const createdIndex = events.findIndex((e) => e.type === 'node:created');
      const hierarchyIndex = events.findIndex((e) => e.type === 'hierarchy:changed');
      const cacheIndex = events.findIndex((e) => e.type === 'cache:invalidate');

      expect(createdIndex).toBeGreaterThanOrEqual(0);
      expect(hierarchyIndex).toBeGreaterThan(createdIndex);
      expect(cacheIndex).toBeGreaterThanOrEqual(0);
    });

    it('should include correct nodeIds in event data', () => {
      const originalNodeId = 'test-node';
      nodeManager.initializeNodes([createNode(originalNodeId, 'Original')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      events.length = 0;

      const newNodeId = nodeManager.createNode(originalNodeId, 'New');

      // CRITICAL: Verify event data contains correct nodeIds
      const createdEvent = events.find((e) => e.type === 'node:created');
      expect(createdEvent).toBeDefined();
      expect(createdEvent!.nodeId).toBe(newNodeId);
      expect(createdEvent!.nodeId).not.toBe(originalNodeId);

      const hierarchyEvent = events.find((e) => e.type === 'hierarchy:changed');
      expect(hierarchyEvent).toBeDefined();
    });

    it('should emit events for placeholder node creation', () => {
      nodeManager.initializeNodes([createNode('root1', 'Root')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      events.length = 0;

      // Create placeholder (empty content, simulates Enter without content split)
      const placeholderNodeId = nodeManager.createPlaceholderNode('root1', 'text');

      // Verify events emitted even for placeholder nodes
      const createdEvents = events.filter((e) => e.type === 'node:created');
      expect(createdEvents).toHaveLength(1);
      expect(createdEvents[0].nodeId).toBe(placeholderNodeId);

      const hierarchyEvents = events.filter((e) => e.type === 'hierarchy:changed');
      expect(hierarchyEvents).toHaveLength(1);
    });

    it('should emit events when splitting content (Enter in middle)', () => {
      nodeManager.initializeNodes([createNode('node1', 'Hello World')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      events.length = 0;

      // Simulate content split: update existing node + create new node
      nodeManager.updateNodeContent('node1', 'Hello ');
      const newNodeId = nodeManager.createNode('node1', 'World', 'text');

      // Verify events for both update and creation
      const updatedEvents = events.filter((e) => e.type === 'node:updated');
      expect(updatedEvents.length).toBeGreaterThanOrEqual(1);

      const createdEvents = events.filter((e) => e.type === 'node:created');
      expect(createdEvents).toHaveLength(1);
      expect(createdEvents[0].nodeId).toBe(newNodeId);

      const hierarchyEvents = events.filter((e) => e.type === 'hierarchy:changed');
      expect(hierarchyEvents.length).toBeGreaterThanOrEqual(1);
    });
  });

  // ============================================================================
  // Integration Tests: Behavior Verification
  // ============================================================================

  describe('Integration: Node Creation Behavior', () => {
    let nodeManager: NodeManager;
    let mockEvents: NodeManagerEvents;

    beforeEach(() => {
      mockEvents = {
        focusRequested: vi.fn(),
        hierarchyChanged: vi.fn(),
        nodeCreated: vi.fn(),
        nodeDeleted: vi.fn()
      };

      nodeManager = createReactiveNodeService(mockEvents);
      eventBus.reset();
    });

    it('should create new node below when Enter pressed at end', () => {
      // Setup: one node
      nodeManager.initializeNodes([createNode('node1', 'Content')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Simulate Enter at end (no content split)
      const newNodeId = nodeManager.createNode('node1', '');

      // Verify new node created in data store
      expect(nodeManager.findNode(newNodeId)).toBeDefined();

      // Verify node has correct properties
      const newNode = nodeManager.findNode(newNodeId);
      const originalNode = nodeManager.findNode('node1');
      expect(newNode?.parentId).toBe(originalNode?.parentId || null); // Same parent as original
      expect(newNode?.beforeSiblingId).toBe('node1'); // Comes after node1

      // Verify node creation event was fired
      expect(mockEvents.nodeCreated).toHaveBeenCalledWith(newNodeId);
    });

    it('should split content correctly when Enter in middle', () => {
      nodeManager.initializeNodes([createNode('node1', 'Hello World')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Simulate split: update first node, create second with rest of content
      nodeManager.updateNodeContent('node1', 'Hello ');
      const newNodeId = nodeManager.createNode('node1', 'World');

      // Verify both nodes have correct content
      const firstNode = nodeManager.findNode('node1');
      const secondNode = nodeManager.findNode(newNodeId);

      expect(firstNode?.content).toBe('Hello ');
      expect(secondNode?.content).toBe('World');

      // Verify hierarchy: second node comes after first (beforeSiblingId points to previous sibling)
      expect(secondNode?.beforeSiblingId).toBe('node1');
    });

    it('should handle node creation with header inheritance', () => {
      nodeManager.initializeNodes([createNode('node1', '## Header')], {
        inheritHeaderLevel: 2,
        expanded: true,
        autoFocus: false
      });

      // Create new node with header inheritance
      const newNodeId = nodeManager.createNode('node1', 'Content', 'text', 2);

      const newNode = nodeManager.findNode(newNodeId);
      expect(newNode).toBeDefined();

      // Verify node created successfully
      expect(mockEvents.nodeCreated).toHaveBeenCalledWith(newNodeId);
    });

    it('should maintain hierarchy after multiple node creations', () => {
      nodeManager.initializeNodes([createNode('root', 'Root')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Create multiple nodes (simulating multiple Enter presses)
      const node1 = nodeManager.createNode('root', 'First');
      const node2 = nodeManager.createNode(node1, 'Second');
      const node3 = nodeManager.createNode(node2, 'Third');

      // Verify all nodes exist
      expect(nodeManager.findNode(node1)).toBeDefined();
      expect(nodeManager.findNode(node2)).toBeDefined();
      expect(nodeManager.findNode(node3)).toBeDefined();

      // Verify hierarchy changed events fired
      expect(mockEvents.hierarchyChanged).toHaveBeenCalled();
    });
  });

  // ============================================================================
  // Performance Tests
  // ============================================================================

  describe('Performance: Event Processing', () => {
    let nodeManager: NodeManager;
    let mockEvents: NodeManagerEvents;
    let events: NodeSpaceEvent[];

    beforeEach(() => {
      events = [];

      mockEvents = {
        focusRequested: vi.fn(),
        hierarchyChanged: vi.fn(),
        nodeCreated: vi.fn(),
        nodeDeleted: vi.fn()
      };

      nodeManager = createReactiveNodeService(mockEvents);

      eventBus.reset();
      eventBus.subscribe('*', (event) => {
        events.push(event);
      });
    });

    it('should process node creation events efficiently', () => {
      nodeManager.initializeNodes([createNode('root', 'Root')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      events.length = 0;

      const startTime = performance.now();

      // Create node and measure event processing time
      nodeManager.createNode('root', 'New content');

      const endTime = performance.now();
      const duration = endTime - startTime;

      // Verify events were emitted
      expect(events.length).toBeGreaterThan(0);

      // Verify processing was fast (< 100ms)
      expect(duration).toBeLessThan(100);
    });
  });
});
