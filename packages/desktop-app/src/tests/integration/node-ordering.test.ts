/**
 * Node Ordering Integration Tests
 *
 * Tests that verify the actual visual rendering order of nodes after various operations.
 * These tests would have caught the original bug where beforeSiblingId pointers were correct
 * but nodes appeared in the wrong visual order.
 *
 * CRITICAL: These tests verify user-visible behavior (node order), not implementation details (event payloads).
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { createReactiveNodeService } from '$lib/services/reactiveNodeService.svelte';
import type { Node } from '$lib/types';

describe('Node Ordering Integration Tests', () => {
  let nodeService: ReturnType<typeof createReactiveNodeService>;

  const mockEvents = {
    focusRequested: () => {},
    hierarchyChanged: () => {},
    nodeCreated: () => {},
    nodeDeleted: () => {}
  };

  beforeEach(() => {
    nodeService = createReactiveNodeService(mockEvents);
  });

  function createNode(id: string, content: string, parentId: string | null = null): Node {
    return {
      id,
      nodeType: 'text',
      content,
      parentId,
      originNodeId: parentId || id,
      beforeSiblingId: null,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      properties: {},
      mentions: []
    };
  }

  describe('insertAtBeginning=true Visual Order', () => {
    it('should render new node ABOVE when pressing Enter at beginning', () => {
      // Initialize with two root nodes
      nodeService.initializeNodes([
        createNode('node1', 'First node'),
        createNode('node2', 'Second node')
      ]);

      // Create node above node2 (insertAtBeginning=true)
      const newNodeId = nodeService.createNode('node2', '', 'text', undefined, true);

      // Verify actual rendering order
      const visibleNodes = nodeService.visibleNodes;
      const ids = visibleNodes.map((n) => n.id);

      // CRITICAL: New node should appear BEFORE node2, not after
      expect(ids).toContain(newNodeId);
      const newNodeIndex = ids.indexOf(newNodeId);
      const node2Index = ids.indexOf('node2');

      expect(newNodeIndex).toBeLessThan(node2Index);
      expect(ids).toEqual(['node1', newNodeId, 'node2']);
    });

    it('should maintain correct order with multiple insertAtBeginning operations', () => {
      // Start with a single root node
      nodeService.initializeNodes([createNode('root', 'Root')]);

      // Create multiple nodes at the beginning (simulating pressing Enter at start repeatedly)
      const node1 = nodeService.createNode('root', 'Node 1', 'text', undefined, true);
      const node2 = nodeService.createNode('root', 'Node 2', 'text', undefined, true);
      const node3 = nodeService.createNode('root', 'Node 3', 'text', undefined, true);

      const visibleNodes = nodeService.visibleNodes;
      const ids = visibleNodes.map((n) => n.id);

      // Multiple insertAtBeginning should create stack order (newest on top)
      expect(ids).toEqual([node3, node2, node1, 'root']);
    });

    it('should handle header nodes with insertAtBeginning correctly', () => {
      nodeService.initializeNodes([createNode('header', '# My Header')]);

      // Create empty node above header (simulating Enter at |# My Header)
      const newNodeId = nodeService.createNode('header', '', 'text', 1, true);

      const visibleNodes = nodeService.visibleNodes;
      const ids = visibleNodes.map((n) => n.id);

      expect(ids).toEqual([newNodeId, 'header']);
    });
  });

  describe('Normal Splitting (insertAtBeginning=false) Visual Order', () => {
    it('should render new node AFTER when splitting content in middle', () => {
      nodeService.initializeNodes([createNode('node1', 'First'), createNode('node2', 'Second')]);

      // Create node after node1 (normal split, insertAtBeginning=false)
      const newNodeId = nodeService.createNode('node1', 'New content', 'text', undefined, false);

      const visibleNodes = nodeService.visibleNodes;
      const ids = visibleNodes.map((n) => n.id);

      // New node should appear AFTER node1
      const node1Index = ids.indexOf('node1');
      const newNodeIndex = ids.indexOf(newNodeId);

      expect(newNodeIndex).toBeGreaterThan(node1Index);
      expect(ids).toEqual(['node1', newNodeId, 'node2']);
    });
  });

  describe('Nested Node Ordering', () => {
    it('should maintain correct order for child nodes', () => {
      const parent = createNode('parent', 'Parent');
      const child1 = createNode('child1', 'Child 1', 'parent');
      const child2 = createNode('child2', 'Child 2', 'parent');

      child2.beforeSiblingId = 'child1'; // child2 comes after child1

      nodeService.initializeNodes([parent, child1, child2]);

      // Get children of parent node
      const visibleNodes = nodeService.visibleNodes;
      const parentNode = visibleNodes.find((n) => n.id === 'parent');

      expect(parentNode).toBeDefined();
      expect(parentNode!.children).toEqual(['child1', 'child2']);
    });

    it('should handle insertAtBeginning for child nodes', () => {
      const parent = createNode('parent', 'Parent');
      const child1 = createNode('child1', 'Child 1', 'parent');

      nodeService.initializeNodes([parent, child1], { expanded: true });

      // Create new child node at the beginning (before child1)
      const newChildId = nodeService.createNode('child1', '', 'text', undefined, true);

      const visibleNodes = nodeService.visibleNodes;
      const parentNode = visibleNodes.find((n) => n.id === 'parent');

      // New child should appear before child1 in the children array
      expect(parentNode!.children).toEqual([newChildId, 'child1']);
    });

    it('should maintain deep hierarchy ordering correctly', () => {
      // Create a 3-level hierarchy
      const root = createNode('root', 'Root');
      const child = createNode('child', 'Child', 'root');
      const grandchild1 = createNode('gc1', 'Grandchild 1', 'child');
      const grandchild2 = createNode('gc2', 'Grandchild 2', 'child');

      grandchild2.beforeSiblingId = 'gc1';

      nodeService.initializeNodes([root, child, grandchild1, grandchild2], { expanded: true });

      const visibleNodes = nodeService.visibleNodes;

      // Verify children are in correct order at each level
      const rootNode = visibleNodes.find((n) => n.id === 'root');
      const childNode = visibleNodes.find((n) => n.id === 'child');

      expect(rootNode!.children).toEqual(['child']);
      expect(childNode!.children).toEqual(['gc1', 'gc2']);
    });
  });

  describe('Mixed Operations', () => {
    it('should handle mix of insertAtBeginning and normal splits', () => {
      nodeService.initializeNodes([createNode('node1', 'Node 1')]);

      // Normal split (after)
      const node2 = nodeService.createNode('node1', 'Node 2', 'text', undefined, false);

      // Insert at beginning (before node1)
      const node0 = nodeService.createNode('node1', 'Node 0', 'text', undefined, true);

      // Normal split (after node2)
      const node3 = nodeService.createNode(node2, 'Node 3', 'text', undefined, false);

      const visibleNodes = nodeService.visibleNodes;
      const ids = visibleNodes.map((n) => n.id);

      expect(ids).toEqual([node0, 'node1', node2, node3]);
    });
  });

  describe('Data Corruption Resilience', () => {
    it('should handle orphaned nodes gracefully', () => {
      const node1 = createNode('node1', 'Node 1');
      const node2 = createNode('node2', 'Node 2');
      const orphan = createNode('orphan', 'Orphan');

      // Create orphaned node: its beforeSiblingId points to non-existent node
      orphan.beforeSiblingId = 'non-existent';

      nodeService.initializeNodes([node1, node2, orphan]);

      const visibleNodes = nodeService.visibleNodes;
      const ids = visibleNodes.map((n) => n.id);

      // Orphaned node should still appear (appended to end)
      expect(ids).toContain('orphan');
    });

    it('should handle circular references without infinite loop', () => {
      const node1 = createNode('node1', 'Node 1');
      const node2 = createNode('node2', 'Node 2');

      // Create circular reference: node1 -> node2 -> node1
      node2.beforeSiblingId = 'node1';
      node1.beforeSiblingId = 'node2';

      // This should not throw or hang
      expect(() => {
        nodeService.initializeNodes([node1, node2]);
        const visibleNodes = nodeService.visibleNodes;
        // Should still return nodes, just may not be in perfect order
        expect(visibleNodes.length).toBeGreaterThan(0);
      }).not.toThrow();
    });
  });
});
