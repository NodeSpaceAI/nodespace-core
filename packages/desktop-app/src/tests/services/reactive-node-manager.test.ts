/**
 * ReactiveNodeManager Test Suite
 *
 * Critical tests for the reactive state synchronization fix
 * Specifically tests Issue #71 where createNode breaks UI updates
 */

import { describe, test, expect, beforeEach } from 'vitest';
import { createReactiveNodeService } from '$lib/services/reactiveNodeService.svelte.js';
import type {
  NodeManagerEvents,
  ReactiveNodeService
} from '$lib/services/reactiveNodeService.svelte.js';

describe('ReactiveNodeService - Reactive State Synchronization', () => {
  let nodeManager: ReactiveNodeService;
  let events: NodeManagerEvents;
  let focusRequestedCalls: Array<{ nodeId: string; position?: number }>;
  let nodeCreatedCalls: string[];
  let nodeDeletedCalls: string[];

  beforeEach(() => {
    // Reset event tracking
    focusRequestedCalls = [];
    nodeCreatedCalls = [];
    nodeDeletedCalls = [];

    // Create mock events
    events = {
      focusRequested: (nodeId: string, position?: number) => {
        focusRequestedCalls.push({ nodeId, position });
      },
      hierarchyChanged: () => {
        // Track hierarchy changes
      },
      nodeCreated: (nodeId: string) => {
        nodeCreatedCalls.push(nodeId);
      },
      nodeDeleted: (nodeId: string) => {
        nodeDeletedCalls.push(nodeId);
      }
    };

    nodeManager = createReactiveNodeService(events);
  });

  describe('CRITICAL BUG FIX: createNode Reactive State Sync', () => {
    test('createNode updates reactive state properly - single root node', () => {
      // Initialize with one root node (matches BaseNodeViewer)
      const legacyNodes = [
        {
          id: 'root1',
          nodeType: 'text',
          content: 'Initial node',
          depth: 0,
          parentId: undefined,
          children: [],
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0,
          metadata: {}
        }
      ];

      nodeManager.initializeFromLegacyData(legacyNodes);

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
      const legacyNodes = [
        {
          id: 'parent1',
          nodeType: 'text',
          content: 'Parent node',
          depth: 0,
          parentId: undefined,
          children: ['child1'],
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0,
          metadata: {}
        },
        {
          id: 'child1',
          nodeType: 'text',
          content: 'Child node',
          depth: 1,
          parentId: 'parent1',
          children: [],
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0,
          metadata: {}
        }
      ];

      nodeManager.initializeFromLegacyData(legacyNodes);

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

      // Verify parent's children array is updated in reactive state
      const parent = nodeManager.findNode('parent1');
      expect(parent?.children).toContain(newNodeId);
    });

    test('createNode maintains autoFocus state correctly', () => {
      const legacyNodes = [
        {
          id: 'node1',
          nodeType: 'text',
          content: 'First node',
          depth: 0,
          parentId: undefined,
          children: [],
          expanded: true,
          autoFocus: true, // Initially has focus
          inheritHeaderLevel: 0,
          metadata: {}
        }
      ];

      nodeManager.initializeFromLegacyData(legacyNodes);

      // Verify initial autoFocus
      expect(nodeManager.findNode('node1')?.autoFocus).toBe(true);

      // Create new node - should clear old focus and set new focus
      const newNodeId = nodeManager.createNode('node1', 'New focused node', 'text');

      // Check autoFocus is transferred correctly in reactive state
      expect(nodeManager.findNode('node1')?.autoFocus).toBe(false);
      expect(nodeManager.findNode(newNodeId)?.autoFocus).toBe(true);

      // Verify this is reflected in visibleNodes (the actual UI data source)
      const visibleNodes = nodeManager.visibleNodes;
      const oldNode = visibleNodes.find((n) => n.id === 'node1');
      const newNode = visibleNodes.find((n) => n.id === newNodeId);

      expect(oldNode?.autoFocus).toBe(false);
      expect(newNode?.autoFocus).toBe(true);
    });

    test('reactive state matches base class state after createNode', () => {
      const legacyNodes = [
        {
          id: 'test1',
          nodeType: 'text',
          content: 'Test node',
          depth: 0,
          parentId: undefined,
          children: [],
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0,
          metadata: {}
        }
      ];

      nodeManager.initializeFromLegacyData(legacyNodes);

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

  describe('Other Operations - Regression Tests', () => {
    test('deleteNode still works correctly', () => {
      const legacyNodes = [
        {
          id: 'node1',
          nodeType: 'text',
          content: 'First',
          depth: 0,
          parentId: undefined,
          children: [],
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0,
          metadata: {}
        },
        {
          id: 'node2',
          nodeType: 'text',
          content: 'Second',
          depth: 0,
          parentId: undefined,
          children: [],
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0,
          metadata: {}
        }
      ];

      nodeManager.initializeFromLegacyData(legacyNodes);
      expect(nodeManager.visibleNodes).toHaveLength(2);

      nodeManager.deleteNode('node1');
      expect(nodeManager.visibleNodes).toHaveLength(1);
      expect(nodeManager.visibleNodes[0].id).toBe('node2');
    });

    test('indentNode still works correctly', () => {
      const legacyNodes = [
        {
          id: 'node1',
          nodeType: 'text',
          content: 'First',
          depth: 0,
          parentId: undefined,
          children: [],
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0,
          metadata: {}
        },
        {
          id: 'node2',
          nodeType: 'text',
          content: 'Second',
          depth: 0,
          parentId: undefined,
          children: [],
          expanded: true,
          autoFocus: false,
          inheritHeaderLevel: 0,
          metadata: {}
        }
      ];

      nodeManager.initializeFromLegacyData(legacyNodes);

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
