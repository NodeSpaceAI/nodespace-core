import { describe, it, expect, beforeEach } from 'vitest';
import {
  applyHasChildCreated,
  applyHasChildUpdated,
  applyHasChildDeleted,
  applyChildPlacement
} from '$lib/services/hierarchy-sync';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';

describe('hierarchy-sync', () => {
  beforeEach(() => {
    structureTree.clear();
  });

  describe('applyHasChildCreated', () => {
    it('uses incoming order when it is a real number', () => {
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c', order: 5 });
      const kids = structureTree.getChildrenWithOrder('p');
      expect(kids[0].order).toBe(5);
    });

    it('honors order: 0 (falsy but valid)', () => {
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c', order: 0 });
      expect(structureTree.getChildrenWithOrder('p')[0].order).toBe(0);
    });

    it('preserves existing order when incoming order is missing and child already exists', () => {
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c', order: 3 });
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c', order: undefined });
      expect(structureTree.getChildrenWithOrder('p')[0].order).toBe(3);
    });

    it('appends at tail (not Date.now(), no jitter) when child is new and order missing', () => {
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c1', order: 10 });
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c2', order: undefined });
      const [first, second] = structureTree.getChildrenWithOrder('p');
      expect(first.nodeId).toBe('c1');
      expect(second.nodeId).toBe('c2');
      // Must NOT be Date.now()-range (>1e12) and must be an exact integer (no jitter)
      expect(second.order).toBeLessThan(1e12);
      expect(second.order).toBeGreaterThan(10);
      // Exact value: lastOrder + 1 = 11 (no Math.random() jitter)
      expect(second.order).toBe(11);
    });

    it('appends at exact integer — calling twice produces identical order (deterministic, no jitter)', () => {
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c1', order: 5 });
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c2', order: undefined });
      const order1 = structureTree.getChildrenWithOrder('p').find((c) => c.nodeId === 'c2')!.order;

      structureTree.clear();
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c1', order: 5 });
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c2', order: undefined });
      const order2 = structureTree.getChildrenWithOrder('p').find((c) => c.nodeId === 'c2')!.order;

      expect(order1).toBe(order2);
    });

    it('produces identical structureTree state for same event through both paths (anti-drift)', () => {
      // Simulate same event through "tauri path" (has stripNodePrefix applied upstream)
      // and "browser path" — both call applyHasChildCreated, results must match
      const tree1 = structureTree;
      applyHasChildCreated(tree1, { parentId: 'parent', childId: 'child', order: 2.5 });
      const tauri = structureTree.getChildrenWithOrder('parent');

      structureTree.clear();
      applyHasChildCreated(structureTree, { parentId: 'parent', childId: 'child', order: 2.5 });
      const browser = structureTree.getChildrenWithOrder('parent');

      expect(tauri).toEqual(browser);
    });
  });

  describe('optimistic-then-event convergence (C3a)', () => {
    it('applyHasChildUpdated reconciles optimistic placement to daemon-supplied order', () => {
      // Simulate the full outdent sequence:
      // 1. Optimistic: moveInMemoryRelationship places child with append integer order
      // 2. Daemon emits relationship:updated with authoritative fractional order
      // 3. applyHasChildUpdated must converge structureTree to daemon order

      // Step 1: initial state — grandparent has [oldParent(2.0), sibling(3.0)]
      applyHasChildCreated(structureTree, { parentId: 'gp', childId: 'oldParent', order: 2.0 });
      applyHasChildCreated(structureTree, { parentId: 'gp', childId: 'sibling', order: 3.0 });

      // Step 2: optimistic outdent moves 'child' from oldParent → gp (append, order=1)
      applyHasChildCreated(structureTree, { parentId: 'oldParent', childId: 'child', order: 1.0 });
      structureTree.removeChild({ parentId: 'oldParent', childId: 'child', order: 0 });
      // Appends at integer: children are [oldParent(2), sibling(3)], so child gets order 4
      applyHasChildCreated(structureTree, { parentId: 'gp', childId: 'child', order: undefined });

      const afterOptimistic = structureTree.getChildrenWithOrder('gp');
      // Optimistic appended at end
      expect(afterOptimistic.at(-1)!.nodeId).toBe('child');

      // Step 3: daemon emits relationship:updated with authoritative order 2.5
      // (child should appear between oldParent and sibling)
      applyHasChildUpdated(structureTree, { parentId: 'gp', childId: 'child', order: 2.5 });

      const afterReconcile = structureTree.getChildrenWithOrder('gp');
      // After reconciliation, child is at 2.5 — between oldParent(2.0) and sibling(3.0)
      const childEntry = afterReconcile.find((c) => c.nodeId === 'child')!;
      expect(childEntry.order).toBe(2.5);
      // Correctly sorted
      const orders = afterReconcile.map((c) => c.order);
      expect(orders).toEqual([...orders].sort((a, b) => a - b));
    });

    it('applyHasChildUpdated is idempotent — applying the same daemon order twice is a no-op', () => {
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c', order: 1.5 });
      applyHasChildUpdated(structureTree, { parentId: 'p', childId: 'c', order: 2.5 });
      applyHasChildUpdated(structureTree, { parentId: 'p', childId: 'c', order: 2.5 });
      expect(structureTree.getChildrenWithOrder('p')[0].order).toBe(2.5);
    });
  });

  describe('applyHasChildUpdated', () => {
    it('updates child order in structureTree', () => {
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c', order: 1 });
      applyHasChildUpdated(structureTree, { parentId: 'p', childId: 'c', order: 5 });
      expect(structureTree.getChildrenWithOrder('p')[0].order).toBe(5);
    });

    it('no-ops when order is missing (logs warning)', () => {
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c', order: 1 });
      applyHasChildUpdated(structureTree, { parentId: 'p', childId: 'c', order: undefined });
      expect(structureTree.getChildrenWithOrder('p')[0].order).toBe(1);
    });
  });

  describe('applyHasChildDeleted', () => {
    it('removes child from structureTree', () => {
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'c', order: 1 });
      applyHasChildDeleted(structureTree, { parentId: 'p', childId: 'c' });
      expect(structureTree.getChildren('p')).toEqual([]);
    });
  });

  describe('applyChildPlacement', () => {
    /** Children of `p` as `[id, order]`, in tree order. */
    const tree = () => structureTree.getChildrenWithOrder('p').map((c) => [c.nodeId, c.order]);

    it("replaces the written edge's local key with the store's", () => {
      structureTree.batchAddRelationships([
        { parentId: 'p', childId: 'a', order: 1 },
        { parentId: 'p', childId: 'b', order: 2 }
      ]);
      // Optimistic local placement between a and b.
      structureTree.addInMemoryRelationship('p', 'x', 1.5);

      applyChildPlacement(structureTree, 'x', { parentId: 'p', order: 1.25, respread: [] });

      expect(tree()).toEqual([
        ['a', 1],
        ['x', 1.25],
        ['b', 2]
      ]);
    });

    it('moves every sibling onto the re-spread keys, so a later event lands in the right slot', () => {
      structureTree.batchAddRelationships([
        { parentId: 'p', childId: 'a', order: 1 },
        { parentId: 'p', childId: 'b', order: 1.0000001 },
        { parentId: 'p', childId: 'c', order: 2 }
      ]);
      structureTree.addInMemoryRelationship('p', 'x', 1.00000005);

      // The store re-spread p's children to 1..=n and placed x after a.
      applyChildPlacement(structureTree, 'x', {
        parentId: 'p',
        order: 1.5,
        respread: [
          { nodeId: 'a', order: 1 },
          { nodeId: 'b', order: 2 },
          { nodeId: 'c', order: 3 }
        ]
      });
      expect(tree()).toEqual([
        ['a', 1],
        ['x', 1.5],
        ['b', 2],
        ['c', 3]
      ]);

      // Another client then places y between b and c in the new key space.
      applyHasChildCreated(structureTree, { parentId: 'p', childId: 'y', order: 2.5 });
      expect(structureTree.getChildren('p')).toEqual(['a', 'x', 'b', 'y', 'c']);
    });

    it('leaves a node the local tree has since moved to another parent alone', () => {
      structureTree.addInMemoryRelationship('p', 'a', 1);
      structureTree.addInMemoryRelationship('q', 'x', 7);

      applyChildPlacement(structureTree, 'x', { parentId: 'p', order: 2, respread: [] });

      expect(structureTree.getParent('x')).toBe('q');
      expect(structureTree.getChildrenWithOrder('q')).toEqual([{ nodeId: 'x', order: 7 }]);
      expect(tree()).toEqual([['a', 1]]);
    });
  });
});
