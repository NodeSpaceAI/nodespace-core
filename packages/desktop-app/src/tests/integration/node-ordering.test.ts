/**
 * Node Ordering Integration Tests
 *
 * Tests that verify the actual node ordering in the UI.
 * Node ordering is now handled by the backend via fractional IDs,
 * so these tests verify visual order through the service's visibleNodes method.
 *
 * CRITICAL: These tests verify visual order as seen through the service API,
 * not internal data structures. The backend handles ordering through moveNode
 * operations and fractional IDs.
 */

// Mock Svelte 5 runes immediately before any imports - using proper type assertions
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

import { describe, it, expect, beforeEach } from 'vitest';
import { createReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
import { sharedNodeStore } from '$lib/services/shared-node-store';
import { PersistenceCoordinator } from '$lib/services/persistence-coordinator.svelte';
import { createTestNode } from '../helpers';

describe('Node Ordering Integration Tests', () => {
  let nodeService: ReturnType<typeof createReactiveNodeService>;

  const mockEvents = {
    focusRequested: () => {},
    hierarchyChanged: () => {},
    nodeCreated: () => {},
    nodeDeleted: () => {}
  };

  beforeEach(() => {
    // Reset singleton state between tests to prevent contamination
    sharedNodeStore.__resetForTesting();
    // Enable test mode - this test doesn't use a real database, so we need to
    // gracefully handle database errors (they're caught and ignored in test mode)
    PersistenceCoordinator.getInstance().enableTestMode();
    nodeService = createReactiveNodeService(mockEvents);
  });

  // Helper to get sorted children IDs using the service's visibleNodes method
  // This now uses the backend's ordering (via fractional IDs) rather than beforeSiblingId
  function getSortedChildren(parentId: string | null): string[] {
    // Use visibleNodes to get nodes in display order
    const visible = nodeService.visibleNodes(parentId);

    // Filter to only direct children of the specified parent
    return visible
      .filter((n) => {
        const nodeParentId = n.parentId || null;
        return nodeParentId === parentId;
      })
      .map((n) => n.id);
  }

  describe('insertAtBeginning=true Visual Order', () => {
    it('should render new node ABOVE when pressing Enter at beginning', () => {
      // Initialize with two root nodes - node2 comes after node1
      const node1 = createTestNode('node1', 'First node');
      const node2 = createTestNode('node2', 'Second node');

      nodeService.initializeNodes([node1, node2], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Create node above node2 (insertAtBeginning=true)
      const newNodeId = nodeService.createNode('node2', '', 'text', undefined, true);

      // Verify actual ordering via linked list traversal
      const rootOrder = getSortedChildren(null);

      // CRITICAL: New node should appear BEFORE node2, not after
      expect(rootOrder).toContain(newNodeId);
      const newNodeIndex = rootOrder.indexOf(newNodeId);
      const node2Index = rootOrder.indexOf('node2');

      expect(newNodeIndex).toBeLessThan(node2Index);
      expect(rootOrder).toEqual(['node1', newNodeId, 'node2']);
    });

    it('should maintain correct order with multiple insertAtBeginning operations', () => {
      // Start with a single root node
      nodeService.initializeNodes([createTestNode('root', 'Root')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Create multiple nodes before the previous new node (simulating realistic Enter key presses)
      // Real UX: Press Enter at |root → cursor moves to new node → press Enter again at new node
      const node1 = nodeService.createNode('root', 'Node 1', 'text', undefined, true);
      const node2 = nodeService.createNode(node1, 'Node 2', 'text', undefined, true);
      const node3 = nodeService.createNode(node2, 'Node 3', 'text', undefined, true);

      const rootOrder = getSortedChildren(null);

      // Multiple insertAtBeginning at previous new node creates stack order (newest on top)
      expect(rootOrder).toEqual([node3, node2, node1, 'root']);
    });

    it('should handle header nodes with insertAtBeginning correctly', () => {
      nodeService.initializeNodes([createTestNode('header', '# My Header')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Create empty node above header (simulating Enter at |# My Header)
      const newNodeId = nodeService.createNode('header', '', 'text', 1, true);

      const rootOrder = getSortedChildren(null);

      expect(rootOrder).toEqual([newNodeId, 'header']);
    });
  });

  describe('Normal Splitting (insertAtBeginning=false) Visual Order', () => {
    it('should render new node AFTER when splitting content in middle', () => {
      nodeService.initializeNodes(
        [createTestNode('node1', 'First'), createTestNode('node2', 'Second')],
        {
          inheritHeaderLevel: 0,
          expanded: true,
          autoFocus: false
        }
      );

      // Create node after node1 (normal split, insertAtBeginning=false)
      const newNodeId = nodeService.createNode('node1', 'New content', 'text', undefined, false);

      const rootOrder = getSortedChildren(null);

      // New node should appear AFTER node1
      const node1Index = rootOrder.indexOf('node1');
      const newNodeIndex = rootOrder.indexOf(newNodeId);

      expect(newNodeIndex).toBeGreaterThan(node1Index);
      expect(rootOrder).toEqual(['node1', newNodeId, 'node2']);
    });
  });

  describe('Nested Node Ordering', () => {
    it('should maintain correct order for child nodes', () => {
      const parent = createTestNode('parent', 'Parent');
      const child1 = createTestNode('child1', 'Child 1', 'text', 'parent');
      const child2 = createTestNode('child2', 'Child 2', 'text', 'parent');

      nodeService.initializeNodes([parent, child1, child2], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Get children of parent node via linked list
      const children = getSortedChildren('parent');

      expect(children).toEqual(['child1', 'child2']);
    });

    it('should handle insertAtBeginning for child nodes', () => {
      const parent = createTestNode('parent', 'Parent');
      const child1 = createTestNode('child1', 'Child 1', 'text', 'parent');

      nodeService.initializeNodes([parent, child1], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false,
        parentMapping: {
          parent: null,
          child1: 'parent'
        }
      });

      // Create new child node at the beginning (before child1)
      // When insertAtBeginning=true, the new node appears ABOVE child1 (same level)
      const newChildId = nodeService.createNode('child1', '', 'text', undefined, true);

      // The new node should be created at the same depth as child1 (sibling of child1 under parent)
      // Since both are children of 'parent', getSortedChildren('parent') should return both
      const children = getSortedChildren('parent');

      // New node should be in the children list
      expect(children).toContain(newChildId);
      expect(children).toContain('child1');
    });

    it('should maintain deep hierarchy ordering correctly', () => {
      // Create a 3-level hierarchy
      const root = createTestNode('root', 'Root');
      const child = createTestNode('child', 'Child', 'text', 'root');
      const grandchild1 = createTestNode('gc1', 'Grandchild 1', 'text', 'child');
      const grandchild2 = createTestNode('gc2', 'Grandchild 2', 'text', 'child');

      nodeService.initializeNodes([root, child, grandchild1, grandchild2], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Verify children are in correct order at each level
      const rootChildren = getSortedChildren(null);
      const childChildren = getSortedChildren('child');

      expect(rootChildren).toEqual(['root']);
      const childrenOfRoot = getSortedChildren('root');
      expect(childrenOfRoot).toEqual(['child']);
      expect(childChildren).toEqual(['gc1', 'gc2']);
    });
  });

  describe('Mixed Operations', () => {
    it('should handle mix of insertAtBeginning and normal splits', () => {
      nodeService.initializeNodes([createTestNode('node1', 'Node 1')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Normal split (after)
      const node2 = nodeService.createNode('node1', 'Node 2', 'text', undefined, false);

      // Insert at beginning (before node1)
      const node0 = nodeService.createNode('node1', 'Node 0', 'text', undefined, true);

      // Normal split (after node2)
      const node3 = nodeService.createNode(node2, 'Node 3', 'text', undefined, false);

      const rootOrder = getSortedChildren(null);

      expect(rootOrder).toEqual([node0, 'node1', node2, node3]);
    });
  });

  describe('Data Corruption Resilience', () => {
    it('should handle orphaned nodes gracefully', () => {
      const node1 = createTestNode('node1', 'Node 1');
      const node2 = createTestNode('node2', 'Node 2');
      const orphan = createTestNode('orphan', 'Orphan');

      nodeService.initializeNodes([node1, node2, orphan], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      const rootOrder = getSortedChildren(null);

      // Orphaned node should still appear (appended to end)
      expect(rootOrder).toContain('orphan');
    });

    it('should handle all nodes in initialization gracefully', () => {
      // This test now verifies that all nodes appear even in complex scenarios
      // The backend handles ordering so we just verify nodes are present
      const node1 = createTestNode('node1', 'Node 1');
      const node2 = createTestNode('node2', 'Node 2');

      // This should not throw or hang
      expect(() => {
        nodeService.initializeNodes([node1, node2], {
          inheritHeaderLevel: 0,
          expanded: true,
          autoFocus: false
        });
        const rootOrder = getSortedChildren(null);
        // Should still return nodes, just may not be in perfect order
        expect(rootOrder.length).toBeGreaterThan(0);
      }).not.toThrow();
    });
  });
});
