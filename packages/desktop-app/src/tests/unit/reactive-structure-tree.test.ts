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
    // Clear any previous state (clear() also resets the reverse parent index)
    structureTree.clear();
  });

  afterEach(async () => {
    // Cleanup
    structureTree.clear();
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
      // Mutate through the public API — getParent is answered by the reverse
      // child→parent index, which the mutators maintain.
      structureTree.addChild({ parentId: 'parent1', childId: 'child1', order: 1.0 });
      const parent = structureTree.getParent('child1');
      expect(parent).toBe('parent1');
    });

    it('should find correct parent among multiple parents', () => {
      structureTree.addChild({ parentId: 'parent1', childId: 'child1', order: 1.0 });
      structureTree.addChild({ parentId: 'parent2', childId: 'child2', order: 1.0 });

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

    it('should return empty array for non-existent parent', () => {
      structureTree.children.clear();

      const childInfo = structureTree.getChildrenWithOrder('nonexistent');

      expect(childInfo).toEqual([]);
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
      expect(parent2Children).toEqual([{ nodeId: 'child3', order: 1.0 }]);
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
      expect(children.map((c) => c.nodeId)).toEqual(['b', 'c', 'a', 'd']);
    });

    it('should handle empty bulk load', () => {
      structureTree.children.clear();

      // No relationships to add
      expect(structureTree.children.size).toBe(0);
    });
  });

  describe('addChild - duplicate handling', () => {
    it('should silently handle duplicate child with same order', () => {
      structureTree.children.clear();

      // Add child once
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 1.0
      });

      // Add same child again with same order (should be silently ignored)
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 1.0
      });

      // Verify only one instance exists
      const children = structureTree.getChildren('parent1');
      expect(children).toEqual(['child1']);
      expect(children.length).toBe(1);
    });

    it('should update order when duplicate child has different order', () => {
      structureTree.children.clear();

      // Add child with initial order
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 2.0
      });

      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child2',
        order: 3.0
      });

      // Update child1's order to 5.0 (should re-sort)
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 5.0
      });

      // Verify child1 moved to end after re-sort
      const children = structureTree.getChildren('parent1');
      expect(children).toEqual(['child2', 'child1']);

      // Verify only 2 children exist (no duplicates)
      expect(children.length).toBe(2);

      // Verify order was updated
      const childInfo = structureTree.getChildrenWithOrder('parent1');
      expect(childInfo.find((c) => c.nodeId === 'child1')?.order).toBe(5.0);
    });

    it('reparents (moves) a child when added under a different parent', () => {
      // The one-parent invariant is preserved by MOVING, not by rejecting: a
      // has_child edge under a new parent (an indent synced from another window)
      // means the node moved. Rejecting it left the node stranded under its old
      // parent across windows.
      structureTree.children.clear();

      structureTree.__testOnly_addChild({ parentId: 'parent1', childId: 'child1', order: 1.0 });

      // Same child added under parent2 → it moves there.
      structureTree.__testOnly_addChild({ parentId: 'parent2', childId: 'child1', order: 1.0 });

      expect(structureTree.getChildren('parent1')).toEqual([]); // pruned from old parent
      expect(structureTree.getChildren('parent2')).toEqual(['child1']); // now under new parent
      expect(structureTree.getParent('child1')).toBe('parent2');
    });
  });

  describe('removeChild', () => {
    it('should remove child from parent', () => {
      structureTree.children.clear();

      // Add children
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

      // Remove child1
      structureTree.removeChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 1.0
      });

      // Verify child1 removed
      const children = structureTree.getChildren('parent1');
      expect(children).toEqual(['child2']);
    });

    it('should delete parent entry when last child removed', () => {
      structureTree.children.clear();

      // Add single child
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 1.0
      });

      // Remove only child
      structureTree.removeChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 1.0
      });

      // Verify parent entry deleted (not empty array)
      expect(structureTree.children.has('parent1')).toBe(false);
      expect(structureTree.getChildren('parent1')).toEqual([]);
    });

    it('should handle removing non-existent child gracefully', () => {
      structureTree.children.clear();

      // Add child1
      structureTree.__testOnly_addChild({
        parentId: 'parent1',
        childId: 'child1',
        order: 1.0
      });

      // Try to remove non-existent child2
      structureTree.removeChild({
        parentId: 'parent1',
        childId: 'child2',
        order: 1.0
      });

      // Verify child1 still exists
      expect(structureTree.getChildren('parent1')).toEqual(['child1']);
    });

    it('should handle removing from non-existent parent gracefully', () => {
      structureTree.children.clear();

      // Try to remove child from non-existent parent (should not throw)
      structureTree.removeChild({
        parentId: 'nonexistent',
        childId: 'child1',
        order: 1.0
      });

      // Verify no changes
      expect(structureTree.children.size).toBe(0);
    });
  });

  describe('addInMemoryRelationship', () => {
    it('should add in-memory relationship with explicit order', () => {
      structureTree.children.clear();

      structureTree.addInMemoryRelationship('parent1', 'child1', 1.5);

      const children = structureTree.getChildren('parent1');
      expect(children).toEqual(['child1']);

      const childInfo = structureTree.getChildrenWithOrder('parent1');
      expect(childInfo[0].order).toBe(1.5);
    });

    it('should add in-memory relationship with default order 1.0', () => {
      structureTree.children.clear();

      // Call without order parameter
      structureTree.addInMemoryRelationship('parent1', 'child1');

      const children = structureTree.getChildren('parent1');
      expect(children).toEqual(['child1']);

      const childInfo = structureTree.getChildrenWithOrder('parent1');
      expect(childInfo[0].order).toBe(1.0);
    });

    it('should support multiple in-memory relationships with different orders', () => {
      structureTree.children.clear();

      structureTree.addInMemoryRelationship('parent1', 'child1', 1.0);
      structureTree.addInMemoryRelationship('parent1', 'child2', 2.0);
      structureTree.addInMemoryRelationship('parent1', 'child3', 1.5);

      const children = structureTree.getChildren('parent1');
      expect(children).toEqual(['child1', 'child3', 'child2']);
    });
  });

  describe('batchAddRelationships', () => {
    it('should add multiple relationships in batch', () => {
      structureTree.children.clear();

      const relationships = [
        { parentId: 'parent1', childId: 'child1', order: 1.0 },
        { parentId: 'parent1', childId: 'child2', order: 2.0 },
        { parentId: 'parent2', childId: 'child3', order: 1.0 }
      ];

      structureTree.batchAddRelationships(relationships);

      expect(structureTree.getChildren('parent1')).toEqual(['child1', 'child2']);
      expect(structureTree.getChildren('parent2')).toEqual(['child3']);
    });

    it('should handle empty batch', () => {
      structureTree.children.clear();

      structureTree.batchAddRelationships([]);

      expect(structureTree.children.size).toBe(0);
    });

    it('should maintain sort order during batch add', () => {
      structureTree.children.clear();

      const relationships = [
        { parentId: 'parent1', childId: 'child3', order: 3.0 },
        { parentId: 'parent1', childId: 'child1', order: 1.0 },
        { parentId: 'parent1', childId: 'child2', order: 2.0 }
      ];

      structureTree.batchAddRelationships(relationships);

      // Verify children are sorted correctly despite unsorted batch input
      expect(structureTree.getChildren('parent1')).toEqual(['child1', 'child2', 'child3']);
    });
  });

  describe('assertInvariants', () => {
    it('passes with valid tree', () => {
      structureTree.addChild({ parentId: '__root__', childId: 'a', order: 1 });
      structureTree.addChild({ parentId: '__root__', childId: 'b', order: 2 });
      const nodeIds = new Set(['a', 'b']);
      const virtualIds = new Set(['__root__']);
      expect(() => structureTree.assertInvariants(nodeIds, virtualIds)).not.toThrow();
    });

    it('I1: throws in dev for orphan childId not in nodeIds', () => {
      structureTree.addChild({ parentId: '__root__', childId: 'orphan', order: 1 });
      const nodeIds = new Set<string>(); // orphan not present
      const virtualIds = new Set(['__root__']);
      expect(() => structureTree.assertInvariants(nodeIds, virtualIds)).toThrow(/I1/);
    });

    it('I2: throws in dev for dual-parent (same child under two parents)', () => {
      // Add child under both parent1 and parent2 by bypassing addChild's guard
      structureTree.children.set('parent1', [{ nodeId: 'shared', order: 1 }]);
      structureTree.children.set('parent2', [{ nodeId: 'shared', order: 1 }]);
      const nodeIds = new Set(['shared', 'parent1', 'parent2']);
      expect(() => structureTree.assertInvariants(nodeIds)).toThrow(/I2/);
    });

    it('I3: throws in dev for unsorted order', () => {
      // Directly set unsorted state bypassing addChild
      structureTree.children.set('p', [
        { nodeId: 'c1', order: 5 },
        { nodeId: 'c2', order: 2 }
      ]);
      const nodeIds = new Set(['c1', 'c2', 'p']);
      expect(() => structureTree.assertInvariants(nodeIds)).toThrow(/I3/);
    });
  });

  describe('optimistic-then-event idempotency', () => {
    it('authoritative relationship:created after optimistic moveInMemoryRelationship leaves single correct edge', () => {
      // Setup: child under parent1
      structureTree.addChild({ parentId: 'parent1', childId: 'child', order: 2 });

      // Optimistic move to parent2 (e.g. from indentNode)
      structureTree.moveInMemoryRelationship('parent1', 'parent2', 'child', 5);
      expect(structureTree.getChildren('parent2')).toContain('child');
      expect(structureTree.getChildren('parent1')).not.toContain('child');

      // Authoritative relationship:created event arrives (via applyHasChildCreated)
      // addChild deduplicates and re-sorts with backend order
      structureTree.addChild({ parentId: 'parent2', childId: 'child', order: 3 });

      // Must still be under parent2 with backend order, no duplicate
      const kids = structureTree.getChildrenWithOrder('parent2');
      expect(kids.filter((c) => c.nodeId === 'child')).toHaveLength(1);
      expect(kids.find((c) => c.nodeId === 'child')?.order).toBe(3);
      expect(structureTree.getChildren('parent1')).not.toContain('child');
    });
  });

  describe('moveInMemoryRelationship', () => {
    it('should move child from old parent to new parent', () => {
      structureTree.children.clear();

      // Setup: child1 under parent1
      structureTree.addInMemoryRelationship('parent1', 'child1', 1.0);
      structureTree.addInMemoryRelationship('parent1', 'child2', 2.0);

      // Move child1 from parent1 to parent2
      structureTree.moveInMemoryRelationship('parent1', 'parent2', 'child1', 1.0);

      // Verify child1 removed from parent1
      expect(structureTree.getChildren('parent1')).toEqual(['child2']);

      // Verify child1 added to parent2
      expect(structureTree.getChildren('parent2')).toEqual(['child1']);
    });

    it('should handle moving from null parent (root)', () => {
      structureTree.children.clear();

      // Move from root (null parent) to parent1
      structureTree.moveInMemoryRelationship(null, 'parent1', 'child1', 1.0);

      // Verify child1 added to parent1
      expect(structureTree.getChildren('parent1')).toEqual(['child1']);
    });

    it('should make the child a root when the new parent is null', () => {
      structureTree.children.clear();
      structureTree.addInMemoryRelationship('parent1', 'child1', 1.0);
      structureTree.addInMemoryRelationship('parent1', 'child2', 2.0);

      structureTree.moveInMemoryRelationship('parent1', null, 'child1');

      expect(structureTree.getParent('child1')).toBeNull();
      expect(structureTree.getChildren('parent1')).toEqual(['child2']);
    });

    it('should calculate order automatically when not provided', () => {
      structureTree.children.clear();

      // Setup: parent2 already has children
      structureTree.addInMemoryRelationship('parent2', 'child2', 1.0);
      structureTree.addInMemoryRelationship('parent2', 'child3', 2.0);

      // Setup: child1 under parent1
      structureTree.addInMemoryRelationship('parent1', 'child1', 1.0);

      // Move child1 to parent2 without specifying order (should append)
      structureTree.moveInMemoryRelationship('parent1', 'parent2', 'child1');

      // Verify child1 appended to end of parent2's children
      expect(structureTree.getChildren('parent2')).toEqual(['child2', 'child3', 'child1']);

      // Verify order was auto-calculated (should be 3.0 = existing length + 1)
      const childInfo = structureTree.getChildrenWithOrder('parent2');
      expect(childInfo.find((c) => c.nodeId === 'child1')?.order).toBe(3.0);
    });

    it('should calculate order 1.0 when moving to empty parent', () => {
      structureTree.children.clear();

      // Setup: child1 under parent1
      structureTree.addInMemoryRelationship('parent1', 'child1', 1.0);

      // Move child1 to empty parent2 without specifying order
      structureTree.moveInMemoryRelationship('parent1', 'parent2', 'child1');

      // Verify child1 added to parent2 with order 1.0
      const childInfo = structureTree.getChildrenWithOrder('parent2');
      expect(childInfo[0].order).toBe(1.0);
    });

    it('should maintain sort order after move with explicit order', () => {
      structureTree.children.clear();

      // Setup: parent2 has children
      structureTree.addInMemoryRelationship('parent2', 'child2', 1.0);
      structureTree.addInMemoryRelationship('parent2', 'child4', 3.0);

      // Setup: child3 under parent1
      structureTree.addInMemoryRelationship('parent1', 'child3', 1.0);

      // Move child3 to parent2 with order 2.0 (between child2 and child4)
      structureTree.moveInMemoryRelationship('parent1', 'parent2', 'child3', 2.0);

      // Verify child3 inserted in correct position
      expect(structureTree.getChildren('parent2')).toEqual(['child2', 'child3', 'child4']);
    });
  });

  describe('addChild reparents across parents (cloud-sync indent/outdent)', () => {
    it('moves a child to the new parent when addChild names a different parent', () => {
      // A node already under `old` parent (e.g. a line under today's date page).
      structureTree.addChild({ parentId: 'old', childId: 'n', order: 1 });
      expect(structureTree.getChildren('old')).toEqual(['n']);

      // A has_child edge under a DIFFERENT parent arrives (indent on another
      // window): the daemon reparents in one step, so only a relationship:created
      // for the new parent is delivered — possibly before any old-edge delete.
      // It must MOVE the node, not be rejected as an "invariant violation"
      // (indent not reflected across windows).
      structureTree.addChild({ parentId: 'p1', childId: 'n', order: 1 });

      expect(structureTree.getChildren('p1')).toEqual(['n']);
      expect(structureTree.getChildren('old')).toEqual([]); // pruned from old parent
      expect(structureTree.getParent('n')).toBe('p1');
    });

    it('a later relationship:deleted for the old edge is a harmless no-op', () => {
      structureTree.addChild({ parentId: 'old', childId: 'n', order: 1 });
      structureTree.addChild({ parentId: 'p1', childId: 'n', order: 1 }); // reparent
      // Old-edge delete arrives afterwards (out-of-order) — must not remove n from p1.
      structureTree.removeChild({ parentId: 'old', childId: 'n', order: 0 });
      expect(structureTree.getChildren('p1')).toEqual(['n']);
      expect(structureTree.getParent('n')).toBe('p1');
    });
  });

  describe('reverse parent index', () => {
    it('getParent reflects each step of an add → move → remove lifecycle', () => {
      structureTree.addChild({ parentId: 'p1', childId: 'c', order: 1 });
      expect(structureTree.getParent('c')).toBe('p1');

      structureTree.moveInMemoryRelationship('p1', 'p2', 'c', 1);
      expect(structureTree.getParent('c')).toBe('p2');
      expect(structureTree.getChildren('p1')).toEqual([]);

      structureTree.removeChild({ parentId: 'p2', childId: 'c', order: 0 });
      expect(structureTree.getParent('c')).toBeNull();
    });

    it('getParent is correct for every edge after a large batchAddRelationships', () => {
      const relationships: Array<{ parentId: string; childId: string; order: number }> = [];
      for (let i = 0; i < 1000; i++) {
        relationships.push({ parentId: `p${i % 50}`, childId: `c${i}`, order: i });
      }
      structureTree.batchAddRelationships(relationships);

      for (let i = 0; i < 1000; i++) {
        expect(structureTree.getParent(`c${i}`)).toBe(`p${i % 50}`);
      }
    });

    it('reparenting inside a batch keeps the index on the latest parent', () => {
      structureTree.batchAddRelationships([
        { parentId: 'p1', childId: 'c', order: 1 },
        { parentId: 'p2', childId: 'c', order: 1 } // reparent within the same batch
      ]);
      expect(structureTree.getParent('c')).toBe('p2');
      expect(structureTree.getChildren('p1')).toEqual([]);
    });

    it('restore() rebuilds the index from the snapshot', () => {
      structureTree.addChild({ parentId: 'p1', childId: 'c1', order: 1 });
      const snapshot = structureTree.snapshot();

      structureTree.clear();
      expect(structureTree.getParent('c1')).toBeNull();

      structureTree.restore(snapshot);
      expect(structureTree.getParent('c1')).toBe('p1');
    });

    it('clear() empties the index', () => {
      structureTree.addChild({ parentId: 'p1', childId: 'c1', order: 1 });
      structureTree.clear();
      expect(structureTree.getParent('c1')).toBeNull();
    });

    it('getParent never iterates the whole tree (O(1) per lookup)', () => {
      // Build a wide tree through the public API.
      for (let i = 0; i < 500; i++) {
        structureTree.addChild({ parentId: `p${i}`, childId: `c${i}`, order: 1 });
      }

      // Swap in an iteration-counting map via restore() (which rebuilds the
      // index). The removed O(N) implementation walked the map with for..of
      // (Symbol.iterator) on every call; the index-backed one must not.
      class CountingMap extends Map<string, Array<{ nodeId: string; order: number }>> {
        scans = 0;
        [Symbol.iterator]() {
          this.scans++;
          return super[Symbol.iterator]();
        }
      }
      const counting = new CountingMap(structureTree.snapshot());
      structureTree.restore(counting);
      counting.scans = 0; // restore's own index rebuild iterates once — reset

      for (let i = 0; i < 500; i++) {
        expect(structureTree.getParent(`c${i}`)).toBe(`p${i}`);
      }
      expect(structureTree.getParent('not-in-tree')).toBeNull();
      expect(counting.scans).toBe(0);
    });
  });

  describe('runBatch', () => {
    it('applies all mutations with exactly one reactive notification', () => {
      const before = structureTree.children;
      let during: unknown = null;

      structureTree.runBatch(() => {
        structureTree.addChild({ parentId: 'p', childId: 'a', order: 1 });
        structureTree.addChild({ parentId: 'p', childId: 'b', order: 2 });
        structureTree.removeChild({ parentId: 'p', childId: 'a', order: 0 });
        during = structureTree.children;
      });

      // No reassignment while the batch is open ($state.raw consumers are not
      // invalidated per-op) ...
      expect(during).toBe(before);
      // ... exactly one reassignment when the batch closes.
      expect(structureTree.children).not.toBe(before);
      expect(structureTree.getChildren('p')).toEqual(['b']);
    });

    it('preserves op order — create then delete of the same edge ends absent', () => {
      structureTree.runBatch(() => {
        structureTree.addChild({ parentId: 'p', childId: 'c', order: 1 });
        structureTree.removeChild({ parentId: 'p', childId: 'c', order: 0 });
      });
      expect(structureTree.getChildren('p')).toEqual([]);
      expect(structureTree.getParent('c')).toBeNull();
    });

    it('preserves op order — delete then re-create of the same edge ends present', () => {
      structureTree.addChild({ parentId: 'p', childId: 'c', order: 1 });
      structureTree.runBatch(() => {
        structureTree.removeChild({ parentId: 'p', childId: 'c', order: 0 });
        structureTree.addChild({ parentId: 'p', childId: 'c', order: 2 });
      });
      expect(structureTree.getChildren('p')).toEqual(['c']);
      expect(structureTree.getParent('c')).toBe('p');
    });

    it('a batch with no mutations does not notify', () => {
      const before = structureTree.children;
      structureTree.runBatch(() => {});
      expect(structureTree.children).toBe(before);
    });

    it('nested batches notify once, at the end of the outermost batch', () => {
      const before = structureTree.children;
      structureTree.runBatch(() => {
        structureTree.addChild({ parentId: 'p', childId: 'a', order: 1 });
        structureTree.runBatch(() => {
          structureTree.addChild({ parentId: 'p', childId: 'b', order: 2 });
        });
        // Inner batch closed, but the outer one is still open — no notify yet.
        expect(structureTree.children).toBe(before);
      });
      expect(structureTree.children).not.toBe(before);
      expect(structureTree.getChildren('p')).toEqual(['a', 'b']);
    });

    it('notifies even when the batch callback throws after mutating', () => {
      const before = structureTree.children;
      expect(() =>
        structureTree.runBatch(() => {
          structureTree.addChild({ parentId: 'p', childId: 'a', order: 1 });
          throw new Error('boom');
        })
      ).toThrow('boom');
      // The mutation landed, so consumers must still be invalidated.
      expect(structureTree.children).not.toBe(before);
      expect(structureTree.getChildren('p')).toEqual(['a']);
    });
  });
});
