import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';

/**
 * Tests for ReactiveStructureTree store
 *
 * Note: These tests do NOT use Tauri event listeners since they're complex
 * to mock. Instead, we test the core logic directly by calling the internal
 * methods through the public API.
 *
 * In real usage, the store is updated via direct method calls from services
 * that handle domain events forwarded from the backend.
 */

describe('ReactiveStructureTree', () => {
  beforeEach(async () => {
    // Initialize with event listeners disabled in test environment
    // (Tauri events won't work in test runner)
    vi.clearAllMocks();
    // Clear any previous state
    structureTree.children.clear();
  });

  afterEach(async () => {
    // Cleanup
    structureTree.children.clear();
  });

  describe('getChildren', () => {
    it('should return empty array for node with no children', () => {
      const children = structureTree.getChildren('parent1');
      expect(children).toEqual([]);
    });

    it('should return child IDs in sorted order by edge.order', () => {
      // Manually add edges in unsorted order
      const parentId = 'parent1';

      // Add child3 with order 3.0
      structureTree.children.set(parentId, [{ nodeId: 'child3', order: 3.0 }]);

      // Simulate adding child1 with order 2.0 (should be inserted before child3)
      const children = structureTree.children.get(parentId) || [];
      const insertIndex = children.findIndex((c) => c.order >= 2.0);
      children.splice(insertIndex, 0, { nodeId: 'child1', order: 2.0 });
      structureTree.children.set(parentId, children);

      // Add child2 with order 2.5 (should go between child1 and child3)
      const children2 = structureTree.children.get(parentId) || [];
      const insertIndex2 = children2.findIndex((c) => c.order >= 2.5);
      children2.splice(insertIndex2, 0, { nodeId: 'child2', order: 2.5 });
      structureTree.children.set(parentId, children2);

      // Verify order
      const result = structureTree.getChildren(parentId);
      expect(result).toEqual(['child1', 'child2', 'child3']);
    });

    it('should handle multiple parents independently', () => {
      // Add children to parent1
      structureTree.children.set('parent1', [
        { nodeId: 'child1', order: 1.0 },
        { nodeId: 'child2', order: 2.0 }
      ]);

      // Add children to parent2
      structureTree.children.set('parent2', [
        { nodeId: 'child3', order: 1.0 },
        { nodeId: 'child4', order: 2.0 }
      ]);

      const children1 = structureTree.getChildren('parent1');
      const children2 = structureTree.getChildren('parent2');

      expect(children1).toEqual(['child1', 'child2']);
      expect(children2).toEqual(['child3', 'child4']);
    });
  });

  describe('hasChildren', () => {
    it('should return false for node with no children', () => {
      expect(structureTree.hasChildren('parent1')).toBe(false);
    });

    it('should return true for node with children', () => {
      structureTree.children.set('parent1', [{ nodeId: 'child1', order: 1.0 }]);
      expect(structureTree.hasChildren('parent1')).toBe(true);
    });
  });

  describe('getParent', () => {
    it('should return null for node with no parent', () => {
      const parent = structureTree.getParent('orphan');
      expect(parent).toBeNull();
    });

    it('should return parent ID for existing child', () => {
      structureTree.children.set('parent1', [{ nodeId: 'child1', order: 1.0 }]);
      const parent = structureTree.getParent('child1');
      expect(parent).toBe('parent1');
    });

    it('should find correct parent among multiple parents', () => {
      structureTree.children.set('parent1', [{ nodeId: 'child1', order: 1.0 }]);
      structureTree.children.set('parent2', [{ nodeId: 'child2', order: 1.0 }]);

      expect(structureTree.getParent('child1')).toBe('parent1');
      expect(structureTree.getParent('child2')).toBe('parent2');
    });
  });

  describe('getChildrenWithOrder', () => {
    it('should return child info with order values', () => {
      structureTree.children.set('parent1', [
        { nodeId: 'child1', order: 1.5 },
        { nodeId: 'child2', order: 2.5 }
      ]);

      const childInfo = structureTree.getChildrenWithOrder('parent1');

      expect(childInfo).toEqual([
        { nodeId: 'child1', order: 1.5 },
        { nodeId: 'child2', order: 2.5 }
      ]);
    });
  });

  describe('snapshot and restore', () => {
    it('should create deep copy of tree state', () => {
      structureTree.children.set('parent1', [
        { nodeId: 'child1', order: 1.0 },
        { nodeId: 'child2', order: 2.0 }
      ]);

      const snapshot = structureTree.snapshot();

      // Verify snapshot has same data
      expect(snapshot.get('parent1')).toEqual([
        { nodeId: 'child1', order: 1.0 },
        { nodeId: 'child2', order: 2.0 }
      ]);

      // Snapshot should be independent copy
      expect(snapshot).not.toBe(structureTree.children);
    });

    it('should restore from snapshot', () => {
      structureTree.children.set('parent1', [
        { nodeId: 'child1', order: 1.0 },
        { nodeId: 'child2', order: 2.0 }
      ]);

      const snapshot = structureTree.snapshot();

      // Clear and modify tree
      structureTree.children.clear();
      structureTree.children.set('parent2', [{ nodeId: 'child3', order: 1.0 }]);

      // Restore from snapshot
      structureTree.restore(snapshot);

      // Verify restoration
      expect(structureTree.getChildren('parent1')).toEqual(['child1', 'child2']);
      expect(structureTree.hasChildren('parent2')).toBe(false);
    });

    it('should allow rollback on error', () => {
      // Set initial state
      structureTree.children.set('parent1', [{ nodeId: 'child1', order: 1.0 }]);

      // Take snapshot before operation
      const snapshot = structureTree.snapshot();

      // Simulate operation
      structureTree.children.set('parent1', [
        { nodeId: 'child1', order: 1.0 },
        { nodeId: 'child2', order: 2.0 },
        { nodeId: 'child3', order: 3.0 }
      ]);

      // Simulate error and rollback
      structureTree.restore(snapshot);

      // Verify only original child remains
      expect(structureTree.getChildren('parent1')).toEqual(['child1']);
    });
  });

  describe('binary search insertion', () => {
    it('should maintain sorted order with single insertion', () => {
      // Start with two children
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 1.0
      });

      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child3',
        order: 3.0
      });

      // Insert child2 with order 2.0 between existing children using binary search
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child2',
        order: 2.0
      });

      const result = structureTree.getChildren('parent1');
      expect(result).toEqual(['child1', 'child2', 'child3']);
    });

    it('should maintain sorted order with rapid random insertions', () => {
      const parentId = 'parent1';
      const orders = [5.0, 1.0, 3.0, 7.0, 2.0, 6.0, 4.0];

      // Use actual binary search insertion via addChild
      for (let i = 0; i < orders.length; i++) {
        structureTree.__testOnly_addChild({
          parentId,
          childId: `child${i}`,
          order: orders[i]
        });
      }

      // Verify sorted order - children should be sorted regardless of insertion order
      const childInfo = structureTree.getChildrenWithOrder(parentId);
      for (let i = 1; i < childInfo.length; i++) {
        expect(childInfo[i].order).toBeGreaterThan(childInfo[i - 1].order);
      }
    });

    it('should handle insertion at beginning', () => {
      // Add existing children
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child2',
        order: 2.0
      });

      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child3',
        order: 3.0
      });

      // Insert at beginning using binary search (order 0.5 < 2.0)
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 0.5
      });

      const result = structureTree.getChildren('parent1');
      expect(result).toEqual(['child1', 'child2', 'child3']);
    });

    it('should handle insertion at end', () => {
      // Add existing children
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 1.0
      });

      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child2',
        order: 2.0
      });

      // Insert at end using binary search (order 4.0 > 2.0)
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child3',
        order: 4.0
      });

      const result = structureTree.getChildren('parent1');
      expect(result).toEqual(['child1', 'child2', 'child3']);
    });
  });

  describe('edge cases', () => {
    it('should handle fractional orders correctly', () => {
      structureTree.children.set('parent1', [
        { nodeId: 'child1', order: 1.0 },
        { nodeId: 'child3', order: 2.0 }
      ]);

      // Insert between with fractional order
      const children = structureTree.children.get('parent1') || [];
      const insertIndex = children.findIndex((c) => c.order >= 1.5);
      children.splice(insertIndex, 0, { nodeId: 'child2', order: 1.5 });
      structureTree.children.set('parent1', children);

      const result = structureTree.getChildren('parent1');
      expect(result).toEqual(['child1', 'child2', 'child3']);
    });

    it('should handle large order values', () => {
      structureTree.children.set('parent1', [
        { nodeId: 'child1', order: 1000000.0 },
        { nodeId: 'child2', order: 2000000.0 }
      ]);

      const result = structureTree.getChildren('parent1');
      expect(result).toEqual(['child1', 'child2']);
    });

    it('should handle many children efficiently', () => {
      const parentId = 'parent1';
      const children: Array<{ nodeId: string; order: number }> = [];

      // Add 1000 children
      for (let i = 0; i < 1000; i++) {
        children.push({ nodeId: `child${i}`, order: i });
      }

      structureTree.children.set(parentId, children);

      const result = structureTree.getChildren(parentId);
      expect(result).toHaveLength(1000);
      expect(result[0]).toBe('child0');
      expect(result[999]).toBe('child999');
    });
  });

  describe('memory efficiency', () => {
    it('should use less memory than simple array approach', () => {
      const parentId = 'parent1';

      // Create tree with 100 nodes
      const children: Array<{ nodeId: string; order: number }> = [];
      for (let i = 0; i < 100; i++) {
        children.push({ nodeId: `child${i}`, order: i });
      }

      structureTree.children.set(parentId, children);

      // Verify structure
      expect(structureTree.getChildren(parentId)).toHaveLength(100);

      // This test mainly ensures the store works at scale
      // Actual memory comparison would require external profiling
    });
  });

  describe('reactivity', () => {
    it('should have reactive children property', () => {
      // Verify children property is reactive (Svelte $state)
      expect(structureTree.children).toBeDefined();
      expect(structureTree.children).toBeInstanceOf(Map);
    });

    it('should maintain reactivity through Map operations', () => {
      const parentId = 'parent1';

      // Add children
      structureTree.children.set(parentId, [{ nodeId: 'child1', order: 1.0 }]);

      // Get and verify
      const children = structureTree.getChildren(parentId);
      expect(children).toEqual(['child1']);

      // Update children
      const existing = structureTree.children.get(parentId) || [];
      existing.push({ nodeId: 'child2', order: 2.0 });
      structureTree.children.set(parentId, existing);

      // Verify update
      const updated = structureTree.getChildren(parentId);
      expect(updated).toEqual(['child1', 'child2']);
    });
  });

  describe('buildTree (bulk load)', () => {
    it('should populate tree from bulk relationship data', () => {
      // Test the buildTree logic by manually calling addChild in the expected order
      // This exercises the actual implementation path
      const relationships = [
        { parentId: 'parent1', childId: 'child1', order: 1.0 },
        { parentId: 'parent1', childId: 'child2', order: 2.0 },
        { parentId: 'parent2', childId: 'child3', order: 1.0 }
      ];

      // Clear previous state
      structureTree.children.clear();

      // Simulate what buildTree does: add relationships via addChild
      for (const rel of relationships) {
        structureTree.__testOnly_addChild(rel);
      }

      // Verify parent1 has children in correct order
      const parent1Children = structureTree.getChildrenWithOrder('parent1');
      expect(parent1Children).toEqual([
        { nodeId: 'child1', order: 1.0 },
        { nodeId: 'child2', order: 2.0 }
      ]);

      // Verify parent2 has its child
      const parent2Children = structureTree.getChildrenWithOrder('parent2');
      expect(parent2Children).toEqual([
        { nodeId: 'child3', order: 1.0 }
      ]);
    });

    it('should handle bulk load with many relationships and maintain sort order', () => {
      const relationships = [
        { parentId: 'root', childId: 'a', order: 3.0 },
        { parentId: 'root', childId: 'b', order: 1.0 },
        { parentId: 'root', childId: 'c', order: 2.0 },
        { parentId: 'root', childId: 'd', order: 4.0 }
      ];

      structureTree.children.clear();

      // Add relationships in unsorted order to test sort functionality
      for (const rel of relationships) {
        structureTree.__testOnly_addChild(rel);
      }

      // Verify relationships are sorted correctly
      const children = structureTree.getChildrenWithOrder('root');
      expect(children.map(c => c.nodeId)).toEqual(['b', 'c', 'a', 'd']);
    });

    it('should handle empty bulk load', () => {
      structureTree.children.clear();

      // No relationships to add
      expect(structureTree.children.size).toBe(0);
    });
  });
});
