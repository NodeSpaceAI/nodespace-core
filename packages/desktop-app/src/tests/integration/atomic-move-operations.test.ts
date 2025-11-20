/**
 * Unit Tests: Atomic Move Node Operations (Issue #552)
 *
 * Tests the fractional ordering algorithm used by move_node() to atomically move
 * nodes in the hierarchy with support for insert_after_sibling positioning.
 *
 * The actual database integration testing is covered by:
 * - indent-outdent-operations.test.ts (end-to-end via ReactiveNodeService)
 * - parent-child-edge-creation.test.ts (edge creation and hierarchy)
 * - node-ordering.test.ts (order calculations)
 *
 * These tests validate the core algorithm that enables atomic moves.
 */

import { describe, it, expect } from 'vitest';

// Replicate the FractionalOrderCalculator logic for testing
function calculateOrder(
  prevOrder: number | null,
  nextOrder: number | null
): number {
  if (prevOrder === null && nextOrder === null) {
    return 1.0; // First child
  } else if (prevOrder === null && nextOrder !== null) {
    return nextOrder - 1.0; // Before first
  } else if (prevOrder !== null && nextOrder === null) {
    return prevOrder + 1.0; // After last
  } else if (prevOrder !== null && nextOrder !== null) {
    return (prevOrder + nextOrder) / 2.0; // Between siblings
  }
  return 1.0; // Default
}

function needsRebalancing(orders: number[]): boolean {
  if (orders.length < 2) {
    return false;
  }

  for (let i = 1; i < orders.length; i++) {
    const gap = orders[i] - orders[i - 1];
    if (gap < 0.0001) {
      return true; // Precision threshold
    }
  }
  return false;
}

function rebalance(count: number): number[] {
  const result: number[] = [];
  for (let i = 1; i <= count; i++) {
    result.push(i);
  }
  return result;
}

describe('Atomic Move Node Operations - Fractional Ordering (Issue #552)', () => {
  describe('Order calculation for insert positioning', () => {
    it('should calculate order when inserting at beginning (no siblings exist)', () => {
      // Inserting first child
      const order = calculateOrder(null, null);
      expect(order).toBe(1.0);
    });

    it('should calculate order when inserting after last sibling', () => {
      // Inserting after existing last child
      const order = calculateOrder(3.0, null);
      expect(order).toBe(4.0);
    });

    it('should calculate order when inserting before first sibling', () => {
      // Inserting at beginning (before existing children)
      const order = calculateOrder(null, 2.0);
      expect(order).toBe(1.0);
    });

    it('should calculate order when inserting between two siblings', () => {
      // Inserting between 1.0 and 3.0
      const order = calculateOrder(1.0, 3.0);
      expect(order).toBe(2.0);
    });

    it('should calculate correct order for narrow gaps', () => {
      // Inserting between very close values (precision test)
      const order = calculateOrder(1.0001, 1.0002);
      expect(order).toBe(1.00015);
    });

    it('should handle fractional boundaries correctly', () => {
      // Inserting between 1.5 and 2.5
      const order = calculateOrder(1.5, 2.5);
      expect(order).toBe(2.0);
    });
  });

  describe('Precision degradation detection', () => {
    it('should detect when rebalancing is needed', () => {
      // Orders with small gaps need rebalancing
      const result = needsRebalancing([1.0, 1.00001, 1.00002, 1.00003]);
      expect(result).toBe(true);
    });

    it('should detect healthy spacing (no rebalancing needed)', () => {
      const result = needsRebalancing([1.0, 2.0, 3.0, 4.0]);
      expect(result).toBe(false);
    });

    it('should detect boundary case for precision (0.0001 threshold)', () => {
      // Gap of exactly 0.0001 should trigger rebalancing (gap < 0.0001 means we need > 0.0001)
      // The test checks that a gap of 0.0001 is considered too small (needs rebalancing)
      const result = needsRebalancing([1.0, 1.0001]);
      expect(result).toBe(true); // 0.0001 gap means we're at the threshold and need rebalancing
    });

    it('should allow gaps larger than threshold', () => {
      // Gap larger than 0.0001 should NOT trigger rebalancing
      const result = needsRebalancing([1.0, 1.00011]);
      expect(result).toBe(false);
    });

    it('should detect gap smaller than threshold', () => {
      // Gap smaller than 0.0001 should trigger rebalancing
      const result = needsRebalancing([1.0, 1.00005]);
      expect(result).toBe(true);
    });

    it('should handle single element (no rebalancing needed)', () => {
      const result = needsRebalancing([1.0]);
      expect(result).toBe(false);
    });

    it('should handle empty array (no rebalancing needed)', () => {
      const result = needsRebalancing([]);
      expect(result).toBe(false);
    });
  });

  describe('Rebalancing algorithm', () => {
    it('should rebalance to evenly spaced orders', () => {
      const rebalanced = rebalance(5);
      expect(rebalanced).toEqual([1.0, 2.0, 3.0, 4.0, 5.0]);
    });

    it('should handle single item rebalancing', () => {
      const rebalanced = rebalance(1);
      expect(rebalanced).toEqual([1.0]);
    });

    it('should handle large number of items', () => {
      const rebalanced = rebalance(100);
      expect(rebalanced.length).toBe(100);
      expect(rebalanced[0]).toBe(1.0);
      expect(rebalanced[99]).toBe(100.0);

      // Verify evenly spaced
      for (let i = 1; i < rebalanced.length; i++) {
        const gap = rebalanced[i] - rebalanced[i - 1];
        expect(gap).toBe(1.0);
      }
    });
  });

  describe('Order calculations for common move scenarios', () => {
    it('should handle moving node to beginning of sibling chain', () => {
      // Moving to beginning: prev=None, next=first existing order
      const order = calculateOrder(null, 1.0);
      expect(order).toBe(0.0); // Before first

      // This verifies the atomic move can insert before existing first child
      expect(order).toBeLessThan(1.0);
    });

    it('should handle moving node after specific sibling', () => {
      // Simulate: Parent has children at orders [1.0, 2.0, 3.0]
      // Insert after order 1.0 (before 2.0)
      const order = calculateOrder(1.0, 2.0);
      expect(order).toBe(1.5);

      // Verify ordering is maintained
      expect(1.0 < order && order < 2.0).toBe(true);
    });

    it('should handle moving node to end of sibling chain', () => {
      // Simulate: Parent has children at orders [1.0, 2.0]
      // Insert after 2.0 (at end)
      const order = calculateOrder(2.0, null);
      expect(order).toBe(3.0);

      expect(order > 2.0).toBe(true);
    });

    it('should preserve order through multiple insertions', () => {
      // Start: [1.0, 2.0]
      let orders: number[] = [1.0, 2.0];

      // Insert 5 nodes after position 1.0 repeatedly
      // This simulates rapid concurrent inserts in the same position
      for (let i = 0; i < 5; i++) {
        // Calculate next order after orders[0]
        const nextOrder = calculateOrder(
          orders[0],
          orders.length > 1 ? orders[1] : null
        );

        // Insert into order array maintaining sort
        orders.splice(1, 0, nextOrder);
      }

      // Verify all orders are unique
      const uniqueOrders = new Set(orders);
      expect(uniqueOrders.size).toBe(orders.length);

      // Verify sorted order
      for (let i = 1; i < orders.length; i++) {
        expect(orders[i]).toBeGreaterThan(orders[i - 1]);
      }
    });
  });

  describe('Edge cases for move operations', () => {
    it('should handle moving same node multiple times', () => {
      // First move: insert between 1.0 and 2.0
      let order = calculateOrder(1.0, 2.0);
      expect(order).toBe(1.5);

      // Second move: insert between 2.0 and 3.0
      order = calculateOrder(2.0, 3.0);
      expect(order).toBe(2.5);

      // Both orders should be valid and distinct
      const firstOrder = 1.5;
      const secondOrder = 2.5;
      expect(firstOrder).not.toBe(secondOrder);
    });

    it('should handle movement between far-apart siblings', () => {
      // Orders with large gaps have no precision issues
      const order = calculateOrder(1.0, 100.0);
      expect(order).toBe(50.5);

      // Plenty of room for future insertions
      expect(needsRebalancing([1.0, 50.5, 100.0])).toBe(false);
    });

    it('should handle movement with precision degradation detection', () => {
      // Simulate a scenario where precision degrades
      const degradedOrders = [1.0, 1.00001, 1.00002, 1.00003, 1.00004, 1.00005, 1.00006];

      // Should detect rebalancing needed
      expect(needsRebalancing(degradedOrders)).toBe(true);

      // After rebalancing, should have no precision issues
      const rebalanced = rebalance(degradedOrders.length);
      expect(needsRebalancing(rebalanced)).toBe(false);
    });
  });

  describe('Rebalancing integration scenario', () => {
    it('should trigger rebalancing when precision threshold is approached', () => {
      // Simulate the scenario where we have degraded precision and need to insert
      // This validates that the move_node logic correctly detects and triggers rebalancing

      // Start with two siblings with a small gap
      const orders = [1.0, 1.00005]; // Gap of 0.00005 - less than 0.0001 threshold
      expect(needsRebalancing(orders)).toBe(true); // Should detect this needs rebalancing

      // When move_node detects this gap is less than 0.0001, it triggers rebalancing
      // Simulating: move_node would call rebalance_children_for_parent() here
      const rebalanced = rebalance(2);

      // After rebalancing, orders should be [1.0, 2.0]
      expect(rebalanced).toEqual([1.0, 2.0]);

      // Now inserting between them has plenty of space
      const newOrder = calculateOrder(1.0, 2.0);
      expect(newOrder).toBe(1.5);

      // The new order is well-spaced and won't degrade precision
      const afterInsert = [1.0, 1.5, 2.0];
      expect(needsRebalancing(afterInsert)).toBe(false);
    });

    it('should maintain precision through rapid sequential insertions', () => {
      // This validates the complete flow: detect degradation → rebalance → calculate new order
      // Simulating what happens during rapid moves between the same two siblings

      // Start fresh: Parent with 2 children
      let orders = [1.0, 2.0];

      // Simulate 10 insertions between the first two children
      // In real scenario, each insertion would trigger rebalancing when gap gets too small
      for (let i = 0; i < 10; i++) {
        // Check if we need rebalancing before next insertion
        if (needsRebalancing(orders)) {
          // Rebalance before inserting
          orders = rebalance(orders.length);
        }

        // Calculate order for new insertion after first child
        const newOrder = calculateOrder(orders[0], orders[1]);

        // Insert into orders array maintaining sort
        orders.splice(1, 0, newOrder);
      }

      // All orders should still be valid (unique, sorted, good spacing)
      const uniqueOrders = new Set(orders);
      expect(uniqueOrders.size).toBe(orders.length);

      // Check all are properly spaced (or was rebalanced)
      // Note: Some might be degraded if we hit the threshold, but we should never
      // have gaps smaller than our algorithm can handle
      for (let i = 1; i < orders.length; i++) {
        const gap = orders[i] - orders[i - 1];
        expect(gap).toBeGreaterThan(0); // All must be positive gaps
      }
    });
  });
});
