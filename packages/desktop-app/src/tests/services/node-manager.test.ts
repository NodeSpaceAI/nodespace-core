/**
 * NodeManager Test Suite
 *
 * Comprehensive tests for NodeManager service focusing on:
 * - Data migration from legacy array structure
 * - Backspace combination bug fixes
 * - Core node operations
 * - Hierarchy management
 * - Reactive state updates
 */

import { describe, test, expect, beforeEach } from 'vitest';
import {
  ReactiveNodeService as NodeManager,
  type NodeManagerEvents
} from '$lib/services/reactiveNodeService.svelte.js';

describe('NodeManager', () => {
  let nodeManager: NodeManager;
  let events: NodeManagerEvents;
  let focusRequestedCalls: Array<{ nodeId: string; position?: number }>;
  let hierarchyChangedCalls: number;
  let nodeCreatedCalls: string[];
  let nodeDeletedCalls: string[];

  beforeEach(() => {
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

    nodeManager = new NodeManager(events);
  });

  describe('Data Migration', () => {
    test('initializeFromLegacyData converts simple array structure correctly', () => {
      const legacyNodes = [
        {
          id: 'node1',
          type: 'text',
          content: 'First node',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: 'node2',
          type: 'text',
          content: 'Second node',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
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
          type: 'ai-chat',
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
          type: 'text',
          content: 'Parent node',
          autoFocus: false,
          inheritHeaderLevel: 0,
          expanded: true,
          children: [
            {
              id: 'child1',
              type: 'text',
              content: 'First child',
              autoFocus: false,
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            },
            {
              id: 'child2',
              type: 'text',
              content: 'Second child',
              autoFocus: false,
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            }
          ]
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
          type: 'text',
          content: 'Root',
          autoFocus: false,
          inheritHeaderLevel: 0,
          expanded: true,
          children: [
            {
              id: 'level1',
              type: 'text',
              content: 'Level 1',
              autoFocus: false,
              inheritHeaderLevel: 0,
              expanded: true,
              children: [
                {
                  id: 'level2',
                  type: 'text',
                  content: 'Level 2',
                  autoFocus: false,
                  inheritHeaderLevel: 0,
                  expanded: true,
                  children: [
                    {
                      id: 'level3',
                      type: 'text',
                      content: 'Level 3',
                      autoFocus: false,
                      inheritHeaderLevel: 0,
                      children: [],
                      expanded: true
                    }
                  ]
                }
              ]
            }
          ]
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

  describe('Backspace Bug Fix - combineNodes', () => {
    beforeEach(() => {
      // Setup test nodes for combination tests
      const testNodes = [
        {
          id: 'node1',
          type: 'text',
          content: 'First content',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: 'node2',
          type: 'text',
          content: 'Second content',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: 'node3',
          type: 'text',
          content: 'Third content',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [
            {
              id: 'child1',
              type: 'text',
              content: 'Child content',
              autoFocus: false,
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            }
          ],
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
  });

  describe('Core Operations', () => {
    beforeEach(() => {
      const testNodes = [
        {
          id: 'node1',
          type: 'text',
          content: 'Test content',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
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
          type: 'text',
          content: 'Parent',
          autoFocus: false,
          inheritHeaderLevel: 0,
          expanded: true,
          children: []
        },
        {
          id: 'child1',
          type: 'text',
          content: 'Child 1',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: 'child2',
          type: 'text',
          content: 'Child 2',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
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

      let visibleNodes = nodeManager.getVisibleNodes();
      expect(visibleNodes.map((n) => n.id)).toEqual(['parent', 'child1', 'child2']);

      // Collapse parent
      nodeManager.toggleExpanded('parent');

      visibleNodes = nodeManager.getVisibleNodes();
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
          type: 'text',
          content: 'Node 1',
          autoFocus: false,
          inheritHeaderLevel: 0,
          expanded: true,
          children: [
            {
              id: 'child1',
              type: 'text',
              content: 'Child 1',
              autoFocus: false,
              inheritHeaderLevel: 0,
              children: [],
              expanded: true
            }
          ]
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
          type: 'text',
          content: 'Test',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
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
      expect(() => nodeManager.initializeFromLegacyData(malformedData as unknown[])).not.toThrow();
    });

    test('maintains consistency after multiple operations', () => {
      const testNodes = [
        {
          id: 'a',
          type: 'text',
          content: 'A',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: 'b',
          type: 'text',
          content: 'B',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
        },
        {
          id: 'c',
          type: 'text',
          content: 'C',
          autoFocus: false,
          inheritHeaderLevel: 0,
          children: [],
          expanded: true
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
      const visibleNodes = nodeManager.getVisibleNodes();
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
