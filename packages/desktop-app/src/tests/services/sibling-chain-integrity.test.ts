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
import { createReactiveNodeService } from '$lib/services/reactive-node-service.svelte.js';
import type {
  NodeManagerEvents,
  ReactiveNodeService
} from '$lib/services/reactive-node-service.svelte.js';
import type { Node } from '$lib/types/node';

describe('Sibling Chain Integrity', () => {
  let nodeManager: ReactiveNodeService;
  let events: NodeManagerEvents;

  beforeEach(() => {
    // Clear reactive computations from previous tests
    clearDerivedComputations();

    // Create mock events
    events = {
      focusRequested: () => {},
      hierarchyChanged: () => {},
      nodeCreated: () => {},
      nodeDeleted: () => {}
    };

    nodeManager = createReactiveNodeService(events);
  });

  /**
   * Helper to validate sibling chain integrity for a parent
   */
  function validateSiblingChain(parentId: string | null): void {
    const siblings = Array.from(nodeManager.nodes.values()).filter((n) => n.parentId === parentId);

    if (siblings.length === 0) return;

    // Find all nodes claiming to be first child (beforeSiblingId === null)
    const firstChildCandidates = siblings.filter((n) => n.beforeSiblingId === null);
    expect(firstChildCandidates.length).toBe(1); // Must have exactly one first child

    // Build a set of all sibling IDs
    const siblingIds = new Set(siblings.map((n) => n.id));

    // Validate each sibling's beforeSiblingId points to another sibling (or null for first)
    for (const sibling of siblings) {
      if (sibling.beforeSiblingId !== null) {
        expect(siblingIds.has(sibling.beforeSiblingId)).toBe(true);
      }
    }

    // Validate we can traverse the entire chain from first to last
    const firstChild = firstChildCandidates[0];
    const chainOrder: string[] = [firstChild.id];
    let current = firstChild.id;

    while (chainOrder.length < siblings.length) {
      const nextSibling = siblings.find((n) => n.beforeSiblingId === current);
      if (!nextSibling) break;
      chainOrder.push(nextSibling.id);
      current = nextSibling.id;
    }

    // All siblings should be in the chain
    expect(chainOrder.length).toBe(siblings.length);
  }

  describe('Creating nodes between siblings', () => {
    test('createNode updates next sibling when inserting in middle', () => {
      // Setup: Parent with two children
      const parent: Node = {
        id: 'parent',
        content: 'Parent',
        nodeType: 'text',
        parentId: null,
        containerNodeId: 'parent',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const child1: Node = {
        id: 'child1',
        content: 'Child 1',
        nodeType: 'text',
        parentId: 'parent',
        containerNodeId: 'parent',
        beforeSiblingId: null, // First child
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const child2: Node = {
        id: 'child2',
        content: 'Child 2',
        nodeType: 'text',
        parentId: 'parent',
        containerNodeId: 'parent',
        beforeSiblingId: 'child1', // Second child
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      nodeManager.initializeNodes([parent, child1, child2], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      validateSiblingChain('parent');

      // Act: Create a new node between child1 and child2
      const newNodeId = nodeManager.createNode('child1', 'New Child', 'text');

      // Assert: Sibling chain should be intact
      validateSiblingChain('parent');

      const newNode = nodeManager.findNode(newNodeId);
      const updatedChild2 = nodeManager.findNode('child2');

      expect(newNode?.beforeSiblingId).toBe('child1'); // New node after child1
      expect(updatedChild2?.beforeSiblingId).toBe(newNodeId); // Child2 now after new node
    });

    test('createPlaceholderNode updates next sibling when inserting in middle', () => {
      // Setup: Three siblings
      const sibling1: Node = {
        id: 'sibling1',
        content: 'Sibling 1',
        nodeType: 'text',
        parentId: null,
        containerNodeId: 'sibling1',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const sibling2: Node = {
        id: 'sibling2',
        content: 'Sibling 2',
        nodeType: 'text',
        parentId: null,
        containerNodeId: 'sibling2',
        beforeSiblingId: 'sibling1',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      nodeManager.initializeNodes([sibling1, sibling2], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      validateSiblingChain(null); // Root level

      // Act: Create placeholder after sibling1
      const placeholderId = nodeManager.createPlaceholderNode('sibling1', 'text');

      // Assert: Sibling chain should be intact
      validateSiblingChain(null);

      const placeholder = nodeManager.findNode(placeholderId);
      const updatedSibling2 = nodeManager.findNode('sibling2');

      expect(placeholder?.beforeSiblingId).toBe('sibling1');
      expect(updatedSibling2?.beforeSiblingId).toBe(placeholderId);
    });
  });

  describe('Indenting nodes with multiple siblings', () => {
    test('indentNode with 3+ siblings uses correct previous sibling', () => {
      // Setup: Three siblings at root level
      const nodes: Node[] = [
        {
          id: 'node1',
          content: 'Node 1',
          nodeType: 'text',
          parentId: null,
          containerNodeId: 'node1',
          beforeSiblingId: null, // First
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          properties: {},
          mentions: []
        },
        {
          id: 'node2',
          content: 'Node 2',
          nodeType: 'text',
          parentId: null,
          containerNodeId: 'node2',
          beforeSiblingId: 'node1', // Second
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          properties: {},
          mentions: []
        },
        {
          id: 'node3',
          content: 'Node 3',
          nodeType: 'text',
          parentId: null,
          containerNodeId: 'node3',
          beforeSiblingId: 'node2', // Third
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          properties: {},
          mentions: []
        }
      ];

      nodeManager.initializeNodes(nodes, {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      validateSiblingChain(null);

      // Act: Indent node2 (should become child of node1, NOT node3)
      const success = nodeManager.indentNode('node2');

      // Assert
      expect(success).toBe(true);

      const node2 = nodeManager.findNode('node2');
      expect(node2?.parentId).toBe('node1'); // Should be child of previous sibling (node1)
      expect(node2?.beforeSiblingId).toBe(null); // First child of node1

      // node3 should now point to node1 (node2 was removed from sibling chain)
      const node3 = nodeManager.findNode('node3');
      expect(node3?.beforeSiblingId).toBe('node1');

      // Validate chains
      validateSiblingChain(null); // Root level
      validateSiblingChain('node1'); // node1's children
    });

    test('indenting placeholder created after first node', () => {
      // This is the exact scenario from the bug report
      const parent1: Node = {
        id: 'parent1',
        content: 'Parent 1',
        nodeType: 'text',
        parentId: 'date-page',
        containerNodeId: 'date-page',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const parent2: Node = {
        id: 'parent2',
        content: 'Parent 2',
        nodeType: 'text',
        parentId: 'date-page',
        containerNodeId: 'date-page',
        beforeSiblingId: 'parent1',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const child2: Node = {
        id: 'child2',
        content: 'Child 2',
        nodeType: 'text',
        parentId: 'parent2',
        containerNodeId: 'date-page',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      nodeManager.initializeNodes([parent1, parent2, child2], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      validateSiblingChain('date-page');
      validateSiblingChain('parent2');

      // Act: Create placeholder after parent1 (simulating pressing Enter)
      const placeholderId = nodeManager.createPlaceholderNode('parent1', 'text');

      validateSiblingChain('date-page');

      // Indent the placeholder (simulating pressing Tab)
      const success = nodeManager.indentNode(placeholderId);

      // Assert: Placeholder should be child of parent1, NOT parent2
      expect(success).toBe(true);

      const placeholder = nodeManager.findNode(placeholderId);
      expect(placeholder?.parentId).toBe('parent1'); // Child of parent1
      expect(placeholder?.beforeSiblingId).toBe(null); // First child

      // parent2 should still point to parent1 (placeholder was removed from date-page's children)
      const updatedParent2 = nodeManager.findNode('parent2');
      expect(updatedParent2?.beforeSiblingId).toBe('parent1');

      // Validate all chains
      validateSiblingChain('date-page');
      validateSiblingChain('parent1');
      validateSiblingChain('parent2');
    });

    test('indentNode appends as last child when parent already has children', () => {
      // Setup: parent1 with existing child, then parent2 sibling
      const parent1: Node = {
        id: 'parent1',
        content: 'Parent 1',
        nodeType: 'text',
        parentId: null,
        containerNodeId: 'parent1',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const child1: Node = {
        id: 'child1',
        content: 'Child 1',
        nodeType: 'text',
        parentId: 'parent1',
        containerNodeId: 'parent1',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const parent2: Node = {
        id: 'parent2',
        content: 'Parent 2',
        nodeType: 'text',
        parentId: null,
        containerNodeId: 'parent2',
        beforeSiblingId: 'parent1',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      nodeManager.initializeNodes([parent1, child1, parent2], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      validateSiblingChain(null);
      validateSiblingChain('parent1');

      // Act: Indent parent2 (should become last child of parent1, after child1)
      const success = nodeManager.indentNode('parent2');

      // Assert
      expect(success).toBe(true);

      const parent2Updated = nodeManager.findNode('parent2');
      expect(parent2Updated?.parentId).toBe('parent1');
      expect(parent2Updated?.beforeSiblingId).toBe('child1'); // After child1, not first

      // Validate chains
      validateSiblingChain(null);
      validateSiblingChain('parent1');
    });
  });

  describe('Outdenting and deleting nodes', () => {
    test('outdentNode maintains sibling chain integrity', () => {
      const parent: Node = {
        id: 'parent',
        content: 'Parent',
        nodeType: 'text',
        parentId: null,
        containerNodeId: 'parent',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const child1: Node = {
        id: 'child1',
        content: 'Child 1',
        nodeType: 'text',
        parentId: 'parent',
        containerNodeId: 'parent',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const child2: Node = {
        id: 'child2',
        content: 'Child 2',
        nodeType: 'text',
        parentId: 'parent',
        containerNodeId: 'parent',
        beforeSiblingId: 'child1',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      nodeManager.initializeNodes([parent, child1, child2], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      validateSiblingChain('parent');

      // Act: Outdent child1
      const success = nodeManager.outdentNode('child1');

      // Assert
      expect(success).toBe(true);

      const child1Updated = nodeManager.findNode('child1');
      expect(child1Updated?.parentId).toBe(null); // Now at root
      expect(child1Updated?.beforeSiblingId).toBe('parent'); // After parent

      // child2 should now be first child of parent (child1 was removed)
      const child2Updated = nodeManager.findNode('child2');
      expect(child2Updated?.beforeSiblingId).toBe(null);

      validateSiblingChain(null);
      validateSiblingChain('parent');
    });

    test('outdentNode transfers siblings below as children', () => {
      // This is the exact scenario we just tested with the real database
      const parent1: Node = {
        id: 'parent1',
        content: 'Parent 1',
        nodeType: 'text',
        parentId: 'date-page',
        containerNodeId: 'date-page',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const child1: Node = {
        id: 'child1',
        content: 'Child 1',
        nodeType: 'text',
        parentId: 'parent1',
        containerNodeId: 'parent1',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const parent2: Node = {
        id: 'parent2',
        content: 'Parent 2',
        nodeType: 'text',
        parentId: 'parent1',
        containerNodeId: 'parent1',
        beforeSiblingId: 'child1', // Sibling below child1
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const child2: Node = {
        id: 'child2',
        content: 'Child 2',
        nodeType: 'text',
        parentId: 'parent2',
        containerNodeId: 'parent2',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      nodeManager.initializeNodes([parent1, child1, parent2, child2], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      validateSiblingChain('date-page');
      validateSiblingChain('parent1');
      validateSiblingChain('parent2');

      // Act: Outdent child1 (parent2 should transfer as child of child1)
      const success = nodeManager.outdentNode('child1');

      // Assert
      expect(success).toBe(true);

      // child1 should be at date-page level, after parent1
      const child1Updated = nodeManager.findNode('child1');
      expect(child1Updated?.parentId).toBe('date-page');
      expect(child1Updated?.beforeSiblingId).toBe('parent1');

      // parent2 should now be a child of child1 (transferred sibling)
      const parent2Updated = nodeManager.findNode('parent2');
      expect(parent2Updated?.parentId).toBe('child1');
      expect(parent2Updated?.beforeSiblingId).toBe(null); // First child

      // child2 should still be child of parent2
      const child2Updated = nodeManager.findNode('child2');
      expect(child2Updated?.parentId).toBe('parent2');

      // Validate all chains
      validateSiblingChain('date-page');
      validateSiblingChain('parent1');
      validateSiblingChain('child1');
      validateSiblingChain('parent2');
    });

    test('outdentNode with multiple transferred siblings maintains sibling chain integrity', () => {
      // Setup: outdented node has existing children, and multiple siblings to transfer
      const parent: Node = {
        id: 'parent',
        content: 'Parent',
        nodeType: 'text',
        parentId: null,
        containerNodeId: 'parent',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const child1: Node = {
        id: 'child1',
        content: 'Child 1 (to outdent)',
        nodeType: 'text',
        parentId: 'parent',
        containerNodeId: 'parent',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const existingChild: Node = {
        id: 'existing-child',
        content: 'Existing child of child1',
        nodeType: 'text',
        parentId: 'child1',
        containerNodeId: 'child1',
        beforeSiblingId: null,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const sibling1: Node = {
        id: 'sibling1',
        content: 'Sibling 1 (to transfer)',
        nodeType: 'text',
        parentId: 'parent',
        containerNodeId: 'parent',
        beforeSiblingId: 'child1',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const sibling2: Node = {
        id: 'sibling2',
        content: 'Sibling 2 (to transfer)',
        nodeType: 'text',
        parentId: 'parent',
        containerNodeId: 'parent',
        beforeSiblingId: 'sibling1',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      const sibling3: Node = {
        id: 'sibling3',
        content: 'Sibling 3 (to transfer)',
        nodeType: 'text',
        parentId: 'parent',
        containerNodeId: 'parent',
        beforeSiblingId: 'sibling2',
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        mentions: []
      };

      nodeManager.initializeNodes([parent, child1, existingChild, sibling1, sibling2, sibling3], {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      validateSiblingChain(null); // root
      validateSiblingChain('parent');
      validateSiblingChain('child1');

      // Act: Outdent child1
      // Expected: sibling1, sibling2, sibling3 should transfer as children of child1
      // They should form a chain AFTER existing-child
      const success = nodeManager.outdentNode('child1');

      // Assert
      expect(success).toBe(true);

      const child1Updated = nodeManager.findNode('child1');
      expect(child1Updated?.parentId).toBe(null);
      expect(child1Updated?.beforeSiblingId).toBe('parent');

      // Validate transferred siblings form correct chain after existing child
      const existingChildUpdated = nodeManager.findNode('existing-child');
      expect(existingChildUpdated?.parentId).toBe('child1');
      expect(existingChildUpdated?.beforeSiblingId).toBe(null); // Still first child

      const sibling1Updated = nodeManager.findNode('sibling1');
      expect(sibling1Updated?.parentId).toBe('child1');
      expect(sibling1Updated?.beforeSiblingId).toBe('existing-child'); // After existing child

      const sibling2Updated = nodeManager.findNode('sibling2');
      expect(sibling2Updated?.parentId).toBe('child1');
      expect(sibling2Updated?.beforeSiblingId).toBe('sibling1'); // After sibling1

      const sibling3Updated = nodeManager.findNode('sibling3');
      expect(sibling3Updated?.parentId).toBe('child1');
      expect(sibling3Updated?.beforeSiblingId).toBe('sibling2'); // After sibling2

      // Validate sibling chain integrity
      validateSiblingChain(null); // root
      validateSiblingChain('parent');
      validateSiblingChain('child1'); // Should have 4 children in correct order
    });

    test('deleteNode maintains sibling chain integrity', () => {
      const nodes: Node[] = [
        {
          id: 'node1',
          content: 'Node 1',
          nodeType: 'text',
          parentId: null,
          containerNodeId: 'node1',
          beforeSiblingId: null,
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          properties: {},
          mentions: []
        },
        {
          id: 'node2',
          content: 'Node 2',
          nodeType: 'text',
          parentId: null,
          containerNodeId: 'node2',
          beforeSiblingId: 'node1',
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          properties: {},
          mentions: []
        },
        {
          id: 'node3',
          content: 'Node 3',
          nodeType: 'text',
          parentId: null,
          containerNodeId: 'node3',
          beforeSiblingId: 'node2',
          createdAt: new Date().toISOString(),
          modifiedAt: new Date().toISOString(),
          properties: {},
          mentions: []
        }
      ];

      nodeManager.initializeNodes(nodes, {
        expanded: true,
        autoFocus: false,
        inheritHeaderLevel: 0
      });

      validateSiblingChain(null);

      // Act: Delete node2 (middle node)
      nodeManager.deleteNode('node2');

      // Assert: node3 should now point to node1
      const node3Updated = nodeManager.findNode('node3');
      expect(node3Updated?.beforeSiblingId).toBe('node1');

      validateSiblingChain(null);
    });
  });
});
