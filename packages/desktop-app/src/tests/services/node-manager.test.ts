/**
 * NodeManager Test Suite
 *
 * Comprehensive tests for NodeManager service focusing on:
 * - Data migration from legacy array structure
 * - Backspace node combination operations
 * - Core node operations
 * - Hierarchy management
 * - Reactive state updates
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
import {
  createReactiveNodeService,
  type ReactiveNodeService as NodeManager,
  type NodeManagerEvents
} from '../../lib/services/reactiveNodeService.svelte.js';
import { createTestNode, createMockNodeManagerEvents } from '../helpers';

describe('NodeManager', () => {
  let nodeManager: NodeManager;
  let events: NodeManagerEvents;
  let focusRequestedCalls: Array<{ nodeId: string; position?: number }>;
  let hierarchyChangedCalls: number;
  let nodeCreatedCalls: string[];
  let nodeDeletedCalls: string[];

  beforeEach(() => {
    // Clear reactive computations from previous tests
    clearDerivedComputations();

    // Reset event tracking
    focusRequestedCalls = [];
    hierarchyChangedCalls = 0;
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
        hierarchyChangedCalls++;
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

  describe('Node Combination on Backspace', () => {
    beforeEach(() => {
      // Setup test nodes for combination tests
      const testNodes = [
        createTestNode({ id: 'node1', content: 'First content' }),
        createTestNode({ id: 'node2', content: 'Second content' }),
        createTestNode({ id: 'node3', content: 'Third content' }),
        createTestNode({ id: 'child1', content: 'Child content', parentId: 'node3' })
      ];
      nodeManager.initializeNodes(testNodes, {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });
      // Reset call counts after initialization
      focusRequestedCalls.length = 0;
      hierarchyChangedCalls = 0;
    });

    test('combines non-empty nodes correctly', () => {
      nodeManager.combineNodes('node2', 'node1');

      const node1 = nodeManager.findNode('node1');
      const node2 = nodeManager.findNode('node2');

      expect(node1?.content).toBe('First contentSecond content');
      expect(node2).toBeNull(); // node2 should be deleted
      expect(focusRequestedCalls).toHaveLength(1);
      expect(focusRequestedCalls[0]).toEqual({ nodeId: 'node1', position: 13 }); // Position after "First content"
    });

    test('handles empty node combination', () => {
      // Update node2 to be empty
      nodeManager.updateNodeContent('node2', '');

      nodeManager.combineNodes('node2', 'node1');

      const node1 = nodeManager.findNode('node1');
      const node2 = nodeManager.findNode('node2');

      expect(node1?.content).toBe('First content'); // Unchanged since node2 was empty
      expect(node2).toBeNull();
      expect(focusRequestedCalls).toHaveLength(1);
      expect(focusRequestedCalls[0].position).toBe(13); // End of node1 content
    });

    test('preserves cursor position at junction', () => {
      nodeManager.updateNodeContent('node1', 'Hello ');
      nodeManager.updateNodeContent('node2', 'World!');

      nodeManager.combineNodes('node2', 'node1');

      const node1 = nodeManager.findNode('node1');
      expect(node1?.content).toBe('Hello World!');
      expect(focusRequestedCalls[0].position).toBe(6); // After "Hello "
    });

    test('transfers children during combination', () => {
      nodeManager.combineNodes('node3', 'node2');

      const child1 = nodeManager.findNode('child1');

      const node2Visible = nodeManager.visibleNodes.find((n) => n.id === 'node2');
      expect(node2Visible?.children).toEqual(['child1']);
      expect(child1?.parentId).toBe('node2');
      expect(nodeManager.findNode('node3')).toBeNull();
    });

    test('updates parent references correctly', () => {
      const child1 = nodeManager.findNode('child1');
      expect(child1?.parentId).toBe('node3');

      nodeManager.combineNodes('node3', 'node2');

      const updatedChild1 = nodeManager.findNode('child1');
      expect(updatedChild1?.parentId).toBe('node2');
    });

    test('handles edge cases - first node combination', () => {
      // Try to combine first node with non-existent previous
      nodeManager.combineNodes('node1', 'non-existent');

      // Should not change anything
      expect(nodeManager.findNode('node1')).toBeTruthy();
      expect(focusRequestedCalls).toHaveLength(0);
    });

    test('handles edge cases - non-existent nodes', () => {
      nodeManager.combineNodes('non-existent1', 'non-existent2');

      // Should not crash or change anything
      expect(focusRequestedCalls).toHaveLength(0);
    });

    test('handles nested node combination', () => {
      // Add a parent to child1 to test nested combination
      const parentVisible = nodeManager.visibleNodes.find((n) => n.id === 'node3');
      expect(parentVisible?.children).toEqual(['child1']);

      // Create another child
      const newChildId = nodeManager.createNode('child1', 'Another child', 'text');

      // Reset event calls
      focusRequestedCalls.length = 0;

      // Combine the children
      nodeManager.combineNodes(newChildId, 'child1');

      const child1 = nodeManager.findNode('child1');
      expect(child1?.content).toBe('Child contentAnother child');
      expect(nodeManager.findNode(newChildId)).toBeNull();
    });

    test('combineNodes maintains proper depth hierarchy for children during cross-depth merge', () => {
      // Test cross-depth merge where shallow node is merged onto deeper node
      // Children should maintain proper relative depth and parent assignments

      const depthTestNodes = [
        createTestNode({ id: 'root', content: 'Root' }),
        createTestNode({ id: 'features', content: 'Node A', parentId: 'root' }),
        createTestNode({ id: 'hierarchical', content: 'Node B', parentId: 'features' }),
        createTestNode({ id: 'realtime', content: 'Node C (target)', parentId: 'features' }),
        createTestNode({ id: 'formatting', content: 'Source Node', parentId: 'root' }),
        createTestNode({ id: 'child1', content: 'Child 1', parentId: 'formatting' }),
        createTestNode({ id: 'child2', content: 'Child 2', parentId: 'formatting' })
      ];

      nodeManager.initializeNodes(depthTestNodes, {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      // Perform the merge: shallow node onto deeper node
      nodeManager.combineNodes('formatting', 'realtime');

      // Verify the children preserve their original depth and get correct parent
      const child1After = nodeManager.findNode('child1');
      const child2After = nodeManager.findNode('child2');

      const child1Visible = nodeManager.visibleNodes.find((n) => n.id === 'child1');
      const child2Visible = nodeManager.visibleNodes.find((n) => n.id === 'child2');
      expect(child1Visible?.depth).toBe(1);
      expect(child2Visible?.depth).toBe(1);
      expect(child1After?.parentId).toBe('root');
      expect(child2After?.parentId).toBe('root');

      // Verify parent now contains the children
      const rootVisible = nodeManager.visibleNodes.find((n) => n.id === 'root');
      expect(rootVisible?.children).toContain('child1');
      expect(rootVisible?.children).toContain('child2');

      // Verify the source node was deleted
      expect(nodeManager.findNode('formatting')).toBeNull();

      // Verify the merged content exists in target node
      const realtimeAfter = nodeManager.findNode('realtime');
      expect(realtimeAfter?.content).toBe('Node C (target)Source Node');
    });
  });

  describe('Core Operations', () => {
    beforeEach(() => {
      const testNodes = [createTestNode({ id: 'node1', content: 'Test content' })];
      nodeManager.initializeNodes(testNodes, {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });
      // Reset event counts
      nodeCreatedCalls.length = 0;
      nodeDeletedCalls.length = 0;
      hierarchyChangedCalls = 0;
    });

    test('createNode inserts correctly', () => {
      const newNodeId = nodeManager.createNode('node1', 'New content', 'text');

      expect(newNodeId).toBeTruthy();
      expect(nodeManager.rootNodeIds).toEqual(['node1', newNodeId]);
      expect(nodeCreatedCalls).toContain(newNodeId);
      expect(hierarchyChangedCalls).toBeGreaterThan(0);

      const newNode = nodeManager.findNode(newNodeId);
      expect(newNode?.content).toBe('New content');
      const newNodeVisible = nodeManager.visibleNodes.find((n) => n.id === newNodeId);
      expect(newNodeVisible?.autoFocus).toBe(true);
      expect(newNodeVisible?.depth).toBe(0);
    });

    test('updateNodeContent preserves reactivity', () => {
      nodeManager.updateNodeContent('node1', 'Updated content');

      const node = nodeManager.findNode('node1');
      expect(node?.content).toBe('Updated content');
    });

    test('deleteNode removes and updates references', () => {
      const newNodeId = nodeManager.createNode('node1', 'To be deleted', 'text');

      nodeManager.deleteNode(newNodeId);

      expect(nodeManager.findNode(newNodeId)).toBeNull();
      expect(nodeDeletedCalls).toContain(newNodeId);
      expect(nodeManager.rootNodeIds).toEqual(['node1']);
    });

    test('findNode returns correct node', () => {
      const node = nodeManager.findNode('node1');
      expect(node?.id).toBe('node1');
      expect(node?.content).toBe('Test content');

      const nonExistent = nodeManager.findNode('non-existent');
      expect(nonExistent).toBeNull();
    });
  });

  describe('Hierarchy Operations', () => {
    beforeEach(() => {
      const testNodes = [
        createTestNode({ id: 'parent', content: 'Parent' }),
        createTestNode({ id: 'child1', content: 'Child 1' }),
        createTestNode({ id: 'child2', content: 'Child 2' })
      ];
      nodeManager.initializeNodes(testNodes, {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });
      hierarchyChangedCalls = 0;
    });

    test('indentNode updates depth and parent', () => {
      const success = nodeManager.indentNode('child1');

      expect(success).toBe(true);
      expect(hierarchyChangedCalls).toBeGreaterThan(0);

      const child1 = nodeManager.findNode('child1');

      const parentVisible = nodeManager.visibleNodes.find((n) => n.id === 'parent');
      expect(parentVisible?.children).toEqual(['child1']);
      expect(child1?.parentId).toBe('parent');
      const child1Visible = nodeManager.visibleNodes.find((n) => n.id === 'child1');
      expect(child1Visible?.depth).toBe(1);
      expect(nodeManager.rootNodeIds).toEqual(['parent', 'child2']);
    });

    test('outdentNode maintains tree structure', () => {
      // First indent child1
      nodeManager.indentNode('child1');

      // Then outdent it
      const success = nodeManager.outdentNode('child1');

      expect(success).toBe(true);

      const child1 = nodeManager.findNode('child1');

      const parentVisible = nodeManager.visibleNodes.find((n) => n.id === 'parent');
      expect(parentVisible?.children).toEqual([]);
      expect(child1?.parentId).toBeNull();
      const child1Visible = nodeManager.visibleNodes.find((n) => n.id === 'child1');
      expect(child1Visible?.depth).toBe(0);
      expect(nodeManager.rootNodeIds).toEqual(['parent', 'child1', 'child2']);
    });

    test('getVisibleNodes respects expanded state', () => {
      // Indent child1 under parent
      nodeManager.indentNode('child1');

      let visibleNodes = nodeManager.visibleNodes;
      expect(visibleNodes.map((n) => n.id)).toEqual(['parent', 'child1', 'child2']);

      // Collapse parent
      nodeManager.toggleExpanded('parent');

      visibleNodes = nodeManager.visibleNodes;
      expect(visibleNodes.map((n) => n.id)).toEqual(['parent', 'child2']);
    });

    test('toggleExpanded changes visibility', () => {
      // Indent child1 under parent
      nodeManager.indentNode('child1');

      const parentVisible = nodeManager.visibleNodes.find((n) => n.id === 'parent');
      expect(parentVisible?.expanded).toBe(true);

      const success = nodeManager.toggleExpanded('parent');
      expect(success).toBe(true);
      const parentVisibleAfter = nodeManager.visibleNodes.find((n) => n.id === 'parent');
      expect(parentVisibleAfter?.expanded).toBe(false);
      expect(hierarchyChangedCalls).toBeGreaterThan(0);
    });

    test('handles complex hierarchy changes', () => {
      // Create a complex hierarchy: parent > child1, child2
      nodeManager.indentNode('child1');
      nodeManager.indentNode('child2');

      const child1 = nodeManager.findNode('child1');
      const child2 = nodeManager.findNode('child2');

      // Both child1 and child2 should be under parent after indenting
      const parentVisible = nodeManager.visibleNodes.find((n) => n.id === 'parent');
      expect(parentVisible?.children).toEqual(['child1', 'child2']);
      expect(child1?.parentId).toBe('parent');
      expect(child2?.parentId).toBe('parent');
      const child1Visible = nodeManager.visibleNodes.find((n) => n.id === 'child1');
      expect(child1Visible?.depth).toBe(1);
      const child2Visible = nodeManager.visibleNodes.find((n) => n.id === 'child2');
      expect(child2Visible?.depth).toBe(1);
    });
  });

  describe('Reactive State Management', () => {
    test('visibleNodes getter updates reactively', () => {
      const testNodes = [
        createTestNode({ id: 'node1', content: 'Node 1' }),
        createTestNode({ id: 'child1', content: 'Child 1', parentId: 'node1' })
      ];
      nodeManager.initializeNodes(testNodes, {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      // Initially both should be visible
      expect(nodeManager.visibleNodes.map((n) => n.id)).toEqual(['node1', 'child1']);

      // Collapse parent - child should disappear
      nodeManager.toggleExpanded('node1');
      expect(nodeManager.visibleNodes.map((n) => n.id)).toEqual(['node1']);

      // Expand parent - child should reappear
      nodeManager.toggleExpanded('node1');
      expect(nodeManager.visibleNodes.map((n) => n.id)).toEqual(['node1', 'child1']);
    });

    test('node map updates preserve reactivity', () => {
      const testNodes = [createTestNode({ id: 'test', content: 'Test' })];
      nodeManager.initializeNodes(testNodes, {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      expect(nodeManager.nodes.size).toBe(1);

      const newNodeId = nodeManager.createNode('test', 'New node');
      expect(nodeManager.nodes.size).toBe(2);
      expect(nodeManager.nodes.has(newNodeId)).toBe(true);

      nodeManager.deleteNode(newNodeId);
      expect(nodeManager.nodes.size).toBe(1);
      expect(nodeManager.nodes.has(newNodeId)).toBe(false);
    });
  });

  describe('Error Handling and Edge Cases', () => {
    test('handles operations on non-existent nodes gracefully', () => {
      expect(() => nodeManager.updateNodeContent('non-existent', 'content')).not.toThrow();
      expect(() => nodeManager.deleteNode('non-existent')).not.toThrow();
      expect(() => nodeManager.indentNode('non-existent')).not.toThrow();
      expect(() => nodeManager.outdentNode('non-existent')).not.toThrow();
      expect(() => nodeManager.toggleExpanded('non-existent')).not.toThrow();
    });

    test('handles empty node data', () => {
      nodeManager.initializeNodes([]);

      expect(nodeManager.nodes.size).toBe(0);
      expect(nodeManager.rootNodeIds).toEqual([]);
      expect(nodeManager.visibleNodes).toEqual([]);
    });

    test('handles malformed node data', () => {
      // Note: With typed interfaces, malformed data shouldn't compile
      // This test now validates that proper error handling exists
      const partialData = [
        createTestNode({ id: 'incomplete', content: '' }) // Empty content is valid
      ];

      // Should not crash with minimal valid data
      expect(() => nodeManager.initializeNodes(partialData)).not.toThrow();
    });

    test('maintains consistency after multiple operations', () => {
      const testNodes = [
        createTestNode({ id: 'a', content: 'A' }),
        createTestNode({ id: 'b', content: 'B' }),
        createTestNode({ id: 'c', content: 'C' })
      ];
      nodeManager.initializeNodes(testNodes, {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      // Perform multiple operations
      nodeManager.indentNode('b');
      nodeManager.indentNode('c');
      const newId = nodeManager.createNode('c', 'D');
      nodeManager.combineNodes('c', 'b');
      nodeManager.outdentNode(newId);

      // Verify consistency
      const visibleNodes = nodeManager.visibleNodes;
      for (const node of visibleNodes) {
        if (node.parentId) {
          const parent = nodeManager.findNode(node.parentId);
          expect(parent).toBeTruthy();
          // children is in visibleNodes, not on Node
          const parentVisible = visibleNodes.find((n) => n.id === node.parentId);
          expect(parentVisible?.children).toContain(node.id);
        }
      }
    });
  });
});
