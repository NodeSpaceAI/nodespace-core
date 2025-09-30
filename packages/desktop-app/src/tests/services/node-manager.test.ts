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

    // Create mock events
    events = {
      focusRequested: (nodeId: string, position?: number) => {
        focusRequestedCalls.push({ nodeId, position });
      },
      hierarchyChanged: () => {
        hierarchyChangedCalls++;
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

  describe('Data Migration', () => {
    test('initializeFromLegacyData converts simple array structure correctly', () => {
      const legacyNodes = [
        {
          id: 'node1',
          nodeType: 'text',
          content: 'First node',
          depth: 0,
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          metadata: {}
        },
        {
          id: 'node2',
          nodeType: 'text',
          content: 'Second node',
          depth: 0,
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          metadata: {}
        }
      ];

      nodeManager.initializeFromLegacyData(legacyNodes);

      expect(nodeManager.nodes.size).toBe(2);
      expect(nodeManager.rootNodeIds).toEqual(['node1', 'node2']);
      expect(hierarchyChangedCalls).toBe(1);

      const node1 = nodeManager.findNode('node1');
      expect(node1).toBeTruthy();
      expect(node1?.content).toBe('First node');
      expect(node1?.nodeType).toBe('text');
      expect(node1?.depth).toBe(0);
      expect(node1?.parentId).toBeUndefined();
    });

    test('preserves all node properties during migration', () => {
      const legacyNodes = [
        {
          id: 'complex-node',
          nodeType: 'ai-chat',
          content: '# Header with **formatting**',
          autoFocus: true,
          inheritHeaderLevel: 1,
          expanded: false,
          metadata: { customProp: 'test-value' },
          children: []
        }
      ];

      nodeManager.initializeFromLegacyData(legacyNodes);

      const node = nodeManager.findNode('complex-node');
      expect(node).toBeTruthy();
      expect(node?.nodeType).toBe('ai-chat');
      expect(node?.content).toBe('# Header with **formatting**');
      expect(node?.autoFocus).toBe(true);
      expect(node?.inheritHeaderLevel).toBe(1);
      expect(node?.expanded).toBe(false);
      expect(node?.metadata.customProp).toBe('test-value');
    });

    test('maintains parent-child relationships', () => {
      const legacyNodes = [
        {
          id: 'parent',
          nodeType: 'text',
          content: 'Parent node',
          autoFocus: false,
          inheritHeaderLevel: 0,
          expanded: true,
          children: ['child1', 'child2']
        },
        {
          id: 'child1',
          nodeType: 'text',
          content: 'First child',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: 'child2',
          nodeType: 'text',
          content: 'Second child',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        }
      ];

      nodeManager.initializeFromLegacyData(legacyNodes);

      const parent = nodeManager.findNode('parent');
      const child1 = nodeManager.findNode('child1');
      const child2 = nodeManager.findNode('child2');

      expect(parent?.children).toEqual(['child1', 'child2']);
      expect(child1?.parentId).toBe('parent');
      expect(child1?.depth).toBe(1);
      expect(child2?.parentId).toBe('parent');
      expect(child2?.depth).toBe(1);
    });

    test('handles nested hierarchy correctly', () => {
      const legacyNodes = [
        {
          id: 'root',
          nodeType: 'text',
          content: 'Root',
          autoFocus: false,
          inheritHeaderLevel: 0,
          expanded: true,
          children: ['level1']
        },
        {
          id: 'level1',
          nodeType: 'text',
          content: 'Level 1',
          autoFocus: false,
          inheritHeaderLevel: 0,
          expanded: true,
          children: ['level2']
        },
        {
          id: 'level2',
          nodeType: 'text',
          content: 'Level 2',
          autoFocus: false,
          inheritHeaderLevel: 0,
          expanded: true,
          children: ['level3']
        },
        {
          id: 'level3',
          nodeType: 'text',
          content: 'Level 3',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        }
      ];

      nodeManager.initializeFromLegacyData(legacyNodes);

      const level3 = nodeManager.findNode('level3');
      expect(level3?.depth).toBe(3);
      expect(level3?.parentId).toBe('level2');

      const level2 = nodeManager.findNode('level2');
      expect(level2?.depth).toBe(2);
      expect(level2?.parentId).toBe('level1');
    });
  });

  describe('Node Combination on Backspace', () => {
    beforeEach(() => {
      // Setup test nodes for combination tests
      const testNodes = [
        {
          id: 'node1',
          nodeType: 'text',
          content: 'First content',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          metadata: {}
        },
        {
          id: 'node2',
          nodeType: 'text',
          content: 'Second content',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          metadata: {}
        },
        {
          id: 'node3',
          nodeType: 'text',
          content: 'Third content',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: ['child1'],
          expanded: true
        },
        {
          id: 'child1',
          nodeType: 'text',
          content: 'Child content',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        }
      ];
      nodeManager.initializeFromLegacyData(testNodes);
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

      const node2 = nodeManager.findNode('node2');
      const child1 = nodeManager.findNode('child1');

      expect(node2?.children).toEqual(['child1']);
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
      const parent = nodeManager.findNode('node3');
      expect(parent?.children).toEqual(['child1']);

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
        // Root node
        {
          id: 'root',
          nodeType: 'text',
          content: 'Root',
          depth: 0,
          parentId: undefined,
          children: ['features']
        },

        {
          id: 'features',
          nodeType: 'text',
          content: 'Node A',
          depth: 1,
          parentId: 'root',
          children: ['hierarchical', 'realtime']
        },

        {
          id: 'hierarchical',
          nodeType: 'text',
          content: 'Node B',
          depth: 2,
          parentId: 'features',
          children: []
        },

        {
          id: 'realtime',
          nodeType: 'text',
          content: 'Node C (target)',
          depth: 3,
          parentId: 'features',
          children: []
        },

        // Shallow node to be merged
        {
          id: 'formatting',
          nodeType: 'text',
          content: 'Source Node',
          depth: 1,
          parentId: 'root',
          children: ['child1', 'child2']
        },

        {
          id: 'child1',
          nodeType: 'text',
          content: 'Child 1',
          depth: 2,
          parentId: 'formatting',
          children: []
        },
        {
          id: 'child2',
          nodeType: 'text',
          content: 'Child 2',
          depth: 2,
          parentId: 'formatting',
          children: []
        }
      ];

      // Initialize with complete node objects
      const completeNodes = depthTestNodes.map((node) => ({
        ...node,
        nodeType: node.nodeType || 'text',
        content: node.content,
        autoFocus: false,
        inheritHeaderLevel: 0,
        expanded: true,
        metadata: {}
      }));
      nodeManager.initializeFromLegacyData(completeNodes);

      // Perform the merge: shallow node onto deeper node
      nodeManager.combineNodes('formatting', 'realtime');

      // Verify the children preserve their original depth and get correct parent
      const child1After = nodeManager.findNode('child1');
      const child2After = nodeManager.findNode('child2');

      expect(child1After?.depth).toBe(1);
      expect(child2After?.depth).toBe(1);
      expect(child1After?.parentId).toBe('root');
      expect(child2After?.parentId).toBe('root');

      // Verify parent now contains the children
      const rootAfter = nodeManager.findNode('root');
      expect(rootAfter?.children).toContain('child1');
      expect(rootAfter?.children).toContain('child2');

      // Verify the source node was deleted
      expect(nodeManager.findNode('formatting')).toBeNull();

      // Verify the merged content exists in target node
      const realtimeAfter = nodeManager.findNode('realtime');
      expect(realtimeAfter?.content).toBe('Node C (target)Source Node');
    });
  });

  describe('Core Operations', () => {
    beforeEach(() => {
      const testNodes = [
        {
          id: 'node1',
          nodeType: 'text',
          content: 'Test content',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          depth: 0,
          metadata: {}
        }
      ];
      nodeManager.initializeFromLegacyData(testNodes);
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
      expect(newNode?.autoFocus).toBe(true);
      expect(newNode?.depth).toBe(0);
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
        {
          id: 'parent',
          nodeType: 'text',
          content: 'Parent',
          autoFocus: false,
          inheritHeaderLevel: 0,
          expanded: true,
          children: []
        },
        {
          id: 'child1',
          nodeType: 'text',
          content: 'Child 1',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          depth: 0,
          metadata: {}
        },
        {
          id: 'child2',
          nodeType: 'text',
          content: 'Child 2',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          depth: 0,
          metadata: {}
        }
      ];
      nodeManager.initializeFromLegacyData(testNodes);
      hierarchyChangedCalls = 0;
    });

    test('indentNode updates depth and parent', () => {
      const success = nodeManager.indentNode('child1');

      expect(success).toBe(true);
      expect(hierarchyChangedCalls).toBeGreaterThan(0);

      const parent = nodeManager.findNode('parent');
      const child1 = nodeManager.findNode('child1');

      expect(parent?.children).toEqual(['child1']);
      expect(child1?.parentId).toBe('parent');
      expect(child1?.depth).toBe(1);
      expect(nodeManager.rootNodeIds).toEqual(['parent', 'child2']);
    });

    test('outdentNode maintains tree structure', () => {
      // First indent child1
      nodeManager.indentNode('child1');

      // Then outdent it
      const success = nodeManager.outdentNode('child1');

      expect(success).toBe(true);

      const parent = nodeManager.findNode('parent');
      const child1 = nodeManager.findNode('child1');

      expect(parent?.children).toEqual([]);
      expect(child1?.parentId).toBeUndefined();
      expect(child1?.depth).toBe(0);
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

      const parent = nodeManager.findNode('parent');
      expect(parent?.expanded).toBe(true);

      const success = nodeManager.toggleExpanded('parent');
      expect(success).toBe(true);
      expect(parent?.expanded).toBe(false);
      expect(hierarchyChangedCalls).toBeGreaterThan(0);
    });

    test('handles complex hierarchy changes', () => {
      // Create a complex hierarchy: parent > child1, child2
      nodeManager.indentNode('child1');
      nodeManager.indentNode('child2');

      const parent = nodeManager.findNode('parent');
      const child1 = nodeManager.findNode('child1');
      const child2 = nodeManager.findNode('child2');

      // Both child1 and child2 should be under parent after indenting
      expect(parent?.children).toEqual(['child1', 'child2']);
      expect(child1?.parentId).toBe('parent');
      expect(child2?.parentId).toBe('parent');
      expect(child1?.depth).toBe(1);
      expect(child2?.depth).toBe(1);
    });
  });

  describe('Reactive State Management', () => {
    test('visibleNodes getter updates reactively', () => {
      const testNodes = [
        {
          id: 'node1',
          nodeType: 'text',
          content: 'Node 1',
          autoFocus: false,
          inheritHeaderLevel: 0,
          expanded: true,
          children: ['child1']
        },
        {
          id: 'child1',
          nodeType: 'text',
          content: 'Child 1',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        }
      ];
      nodeManager.initializeFromLegacyData(testNodes);

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
      const testNodes = [
        {
          id: 'test',
          nodeType: 'text',
          content: 'Test',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          depth: 0,
          metadata: {}
        }
      ];
      nodeManager.initializeFromLegacyData(testNodes);

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

    test('handles empty legacy data', () => {
      nodeManager.initializeFromLegacyData([]);

      expect(nodeManager.nodes.size).toBe(0);
      expect(nodeManager.rootNodeIds).toEqual([]);
      expect(nodeManager.visibleNodes).toEqual([]);
    });

    test('handles malformed legacy data', () => {
      const malformedData = [
        { id: 'incomplete' }, // Missing required fields
        null, // Null entry
        { id: 'child-without-parent', parentId: 'missing' } // Orphaned child
      ];

      // Should not crash
      expect(() => nodeManager.initializeFromLegacyData(malformedData as never[])).not.toThrow();
    });

    test('maintains consistency after multiple operations', () => {
      const testNodes = [
        {
          id: 'a',
          nodeType: 'text',
          content: 'A',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          depth: 0,
          metadata: {}
        },
        {
          id: 'b',
          nodeType: 'text',
          content: 'B',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          depth: 0,
          metadata: {}
        },
        {
          id: 'c',
          nodeType: 'text',
          content: 'C',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true,
          depth: 0,
          metadata: {}
        }
      ];
      nodeManager.initializeFromLegacyData(testNodes);

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
          expect(parent?.children).toContain(node.id);
        }
      }
    });
  });
});
