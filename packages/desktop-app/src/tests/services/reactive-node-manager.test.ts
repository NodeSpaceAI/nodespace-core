/**
 * ReactiveNodeManager Test Suite
 *
 * Critical tests for reactive state synchronization
 * Specifically tests Issue #71 where createNode broke UI updates
 */

// Mock Svelte 5 runes immediately before any imports
import {
  createStateMock,
  createDerivedMock,
  createEffectMock,
  clearDerivedComputations
} from '../utils/svelte-runes-mock.js';

(globalThis as Record<string, unknown>).$state = createStateMock;
(globalThis as Record<string, unknown>).$derived = {
  by: createDerivedMock
};
(globalThis as Record<string, unknown>).$effect = createEffectMock;

import { describe, test, expect, beforeEach } from 'vitest';
import { createReactiveNodeService } from '../../lib/services/reactiveNodeService.svelte.js';
import type {
  NodeManagerEvents,
  ReactiveNodeService
} from '../../lib/services/reactiveNodeService.svelte.js';
import { createTestNode, createMockNodeManagerEvents } from '../helpers';

describe('ReactiveNodeService - Reactive State Synchronization', () => {
  let nodeManager: ReactiveNodeService;
  let events: NodeManagerEvents;
  let focusRequestedCalls: Array<{ nodeId: string; position?: number }>;
  let nodeCreatedCalls: string[];
  let nodeDeletedCalls: string[];

  beforeEach(() => {
    // Clear reactive computations from previous tests
    clearDerivedComputations();

    // Reset event tracking
    focusRequestedCalls = [];
    nodeCreatedCalls = [];
    nodeDeletedCalls = [];

    // Create mock events with custom tracking
    const mockEvents = createMockNodeManagerEvents();
    events = {
      focusRequested: (nodeId: string, position?: number) => {
        focusRequestedCalls.push({ nodeId, position });
        mockEvents.focusRequested(nodeId, position);
      },
      hierarchyChanged: () => {
        // Track hierarchy changes
        mockEvents.hierarchyChanged();
      },
      nodeCreated: (nodeId: string) => {
        nodeCreatedCalls.push(nodeId);
        mockEvents.nodeCreated(nodeId);
      },
      nodeDeleted: (nodeId: string) => {
        nodeDeletedCalls.push(nodeId);
        mockEvents.nodeDeleted(nodeId);
      }
    };

    nodeManager = createReactiveNodeService(events);
  });

  describe('createNode Reactive State Synchronization', () => {
    test('createNode updates reactive state properly - single root node', () => {
      // Initialize with one root node (matches BaseNodeViewer)
      nodeManager.initializeNodes([createTestNode({ id: 'root1', content: 'Initial node' })], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      // BEFORE FIX: visibleNodes would have 1 node
      expect(nodeManager.visibleNodes).toHaveLength(1);
      expect(nodeManager.rootNodeIds).toHaveLength(1);

      // Simulate Enter key - create new node after root1
      const newNodeId = nodeManager.createNode('root1', 'New node content', 'text');

      // AFTER FIX: visibleNodes should have 2 nodes
      expect(nodeManager.visibleNodes).toHaveLength(2);
      expect(nodeManager.rootNodeIds).toHaveLength(2);

      // Verify the new node is in the visible nodes
      const visibleNodes = nodeManager.visibleNodes;
      expect(visibleNodes.some((n) => n.id === newNodeId)).toBe(true);
      expect(visibleNodes.some((n) => n.content === 'New node content')).toBe(true);

      // Verify ordering - new node should come after root1
      expect(visibleNodes[0].id).toBe('root1');
      expect(visibleNodes[1].id).toBe(newNodeId);
    });

    test('createNode updates reactive state properly - with parent-child hierarchy', () => {
      // Initialize with parent-child structure
      nodeManager.initializeNodes(
        [
          createTestNode({ id: 'parent1', content: 'Parent node' }),
          createTestNode({
            id: 'child1',
            content: 'Child node',
            nodeType: 'text',
            parentId: 'parent1'
          })
        ],
        {
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0
        }
      );

      expect(nodeManager.visibleNodes).toHaveLength(2); // parent + child

      // Create new node after child1 (should become child of parent1)
      const newNodeId = nodeManager.createNode('child1', 'New child content', 'text');

      // Should now have 3 visible nodes: parent1, child1, newChild
      expect(nodeManager.visibleNodes).toHaveLength(3);

      const visibleNodes = nodeManager.visibleNodes;
      expect(visibleNodes[0].id).toBe('parent1');
      expect(visibleNodes[1].id).toBe('child1');
      expect(visibleNodes[2].id).toBe(newNodeId);

      // Verify the new node has correct parent
      const newNode = nodeManager.findNode(newNodeId);
      expect(newNode?.parentId).toBe('parent1');

      // Verify parent's children array is computed from parent_id relationships
      // Note: children are computed from visibleNodes, not stored on the node
      const parentInVisible = nodeManager.visibleNodes.find((n) => n.id === 'parent1');
      expect(parentInVisible?.children).toContain(newNodeId);
    });

    test('createNode maintains autoFocus state correctly', () => {
      nodeManager.initializeNodes([createTestNode({ id: 'node1', content: 'First node' })], {
        expanded: true,
        autoFocus: true, // Initially has focus
        inheritHeaderLevel: 0
      });

      // Verify initial autoFocus (in visibleNodes, not on Node itself)
      const initialVisible = nodeManager.visibleNodes.find((n) => n.id === 'node1');
      expect(initialVisible?.autoFocus).toBe(true);

      // Create new node - should clear old focus and set new focus
      const newNodeId = nodeManager.createNode('node1', 'New focused node', 'text');

      // Check autoFocus is transferred correctly in visibleNodes (UI state is not on Node objects)
      // autoFocus is a UI state property, not stored on Node objects returned by findNode

      // Verify this is reflected in visibleNodes (the actual UI data source)
      const visibleNodes = nodeManager.visibleNodes;
      const oldNode = visibleNodes.find((n) => n.id === 'node1');
      const newNode = visibleNodes.find((n) => n.id === newNodeId);

      expect(oldNode?.autoFocus).toBe(false);
      expect(newNode?.autoFocus).toBe(true);
    });

    test('reactive state matches base class state after createNode', () => {
      nodeManager.initializeNodes([createTestNode({ id: 'test1', content: 'Test node' })], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      nodeManager.createNode('test1', 'New test node', 'text');

      // Critical check: reactive state should exactly match base class state
      const baseNodes = Array.from(nodeManager.nodes.values());
      const baseRootIds = nodeManager.rootNodeIds;

      // Check all nodes exist in reactive state
      for (const baseNode of baseNodes) {
        const reactiveNode = nodeManager.findNode(baseNode.id);
        expect(reactiveNode).toEqual(baseNode);
      }

      // Check root IDs match
      expect(nodeManager.rootNodeIds).toEqual(baseRootIds);

      // Check visibleNodes contains all expected nodes
      expect(nodeManager.visibleNodes).toHaveLength(baseNodes.length);
    });
  });

  describe('Bug 4 fix - Collapsed node child transfer', () => {
    test('should not transfer children from collapsed nodes when creating new nodes', () => {
      // Set up parent with children in collapsed state
      nodeManager.initializeNodes(
        [
          createTestNode({ id: 'parent1', content: 'Parent node' }),
          createTestNode({
            id: 'child1',
            content: 'Child 1',
            nodeType: 'text',
            parentId: 'parent1'
          }),
          createTestNode({
            id: 'child2',
            content: 'Child 2',
            nodeType: 'text',
            parentId: 'parent1'
          })
        ],
        {
          expanded: false, // Collapsed - children should not transfer
          autoFocus: false,
          inheritHeaderLevel: 0
        }
      );

      // Create new node after collapsed parent
      const newNodeId = nodeManager.createNode('parent1', 'New node content', 'text');

      // Since parent is collapsed, children should NOT transfer to new node
      // Check children in visibleNodes (they're computed, not stored on Node)
      const visibleNodes = nodeManager.visibleNodes;
      const newNodeVisible = visibleNodes.find((n) => n.id === newNodeId);
      const originalParentVisible = visibleNodes.find((n) => n.id === 'parent1');

      expect(newNodeVisible?.children).toEqual([]); // No children transferred
      expect(originalParentVisible?.children).toEqual(['child1', 'child2']); // Children stay with original

      // Children should still have original parent
      const child1 = nodeManager.findNode('child1');
      const child2 = nodeManager.findNode('child2');
      expect(child1?.parentId).toBe('parent1');
      expect(child2?.parentId).toBe('parent1');
    });

    test('should transfer children from expanded nodes when creating new nodes', () => {
      // Set up parent with children in expanded state
      nodeManager.initializeNodes(
        [
          createTestNode({ id: 'parent1', content: 'Parent node' }),
          createTestNode({
            id: 'child1',
            content: 'Child 1',
            nodeType: 'text',
            parentId: 'parent1'
          }),
          createTestNode({
            id: 'child2',
            content: 'Child 2',
            nodeType: 'text',
            parentId: 'parent1'
          })
        ],
        {
          expanded: true, // Expanded - children should transfer
          autoFocus: false,
          inheritHeaderLevel: 0
        }
      );

      // Create new node after expanded parent
      const newNodeId = nodeManager.createNode('parent1', 'New node content', 'text');

      // Since parent is expanded, children SHOULD transfer to new node
      // Check children in visibleNodes (they're computed, not stored on Node)
      const visibleNodes = nodeManager.visibleNodes;
      const newNodeVisible = visibleNodes.find((n) => n.id === newNodeId);
      const originalParentVisible = visibleNodes.find((n) => n.id === 'parent1');

      expect(newNodeVisible?.children).toEqual(['child1', 'child2']); // Children transferred
      expect(originalParentVisible?.children).toEqual([]); // Original parent now empty

      // Children should now have new node as parent
      const child1 = nodeManager.findNode('child1');
      const child2 = nodeManager.findNode('child2');
      expect(child1?.parentId).toBe(newNodeId);
      expect(child2?.parentId).toBe(newNodeId);
    });
  });

  describe('Other Operations - Regression Tests', () => {
    test('deleteNode still works correctly', () => {
      nodeManager.initializeNodes(
        [
          createTestNode({ id: 'node1', content: 'First' }),
          createTestNode({ id: 'node2', content: 'Second' })
        ],
        {
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0
        }
      );
      expect(nodeManager.visibleNodes).toHaveLength(2);

      nodeManager.deleteNode('node1');
      expect(nodeManager.visibleNodes).toHaveLength(1);
      expect(nodeManager.visibleNodes[0].id).toBe('node2');
    });

    test('indentNode still works correctly', () => {
      nodeManager.initializeNodes(
        [
          createTestNode({ id: 'node1', content: 'First' }),
          createTestNode({ id: 'node2', content: 'Second' })
        ],
        {
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0
        }
      );

      const success = nodeManager.indentNode('node2');
      expect(success).toBe(true);

      // node2 should now be child of node1
      const node2 = nodeManager.findNode('node2');
      expect(node2?.parentId).toBe('node1');

      // Check visible nodes reflect the change
      expect(nodeManager.visibleNodes).toHaveLength(2);
      expect(nodeManager.rootNodeIds).toHaveLength(1);
    });
  });
});
