/**
 * Unit tests for calculateOutdentInsertOrderPure
 *
 * Tests the pure function that calculates fractional order for outdented nodes.
 * This function is critical for correct node ordering after outdent operations.
 *
 * Edge cases tested:
 * - Empty parent children
 * - Missing old parent in children list
 * - Single child (old parent only)
 * - Many children with various positions
 * - Old parent as first/last/middle child
 */

import { describe, it, expect } from 'vitest';
import { calculateOutdentInsertOrderPure } from '$lib/services/reactive-node-service.svelte';

describe('calculateOutdentInsertOrderPure', () => {
  describe('when old parent is found in children list', () => {
    it('returns midpoint between old parent and next sibling', () => {
      const children = [
        { nodeId: 'child-1', order: 1.0 },
        { nodeId: 'old-parent', order: 2.0 },
        { nodeId: 'child-3', order: 3.0 }
      ];

      const result = calculateOutdentInsertOrderPure(children, 'old-parent');

      // Should be midpoint between 2.0 and 3.0
      expect(result).toBe(2.5);
    });

    it('returns old parent order + 1.0 when old parent is last child', () => {
      const children = [
        { nodeId: 'child-1', order: 1.0 },
        { nodeId: 'old-parent', order: 2.0 }
      ];

      const result = calculateOutdentInsertOrderPure(children, 'old-parent');

      // Should be 2.0 + 1.0 = 3.0
      expect(result).toBe(3.0);
    });

    it('handles single child (old parent only)', () => {
      const children = [{ nodeId: 'old-parent', order: 5.0 }];

      const result = calculateOutdentInsertOrderPure(children, 'old-parent');

      // Should be 5.0 + 1.0 = 6.0
      expect(result).toBe(6.0);
    });

    it('returns correct midpoint when old parent is first child with siblings', () => {
      const children = [
        { nodeId: 'old-parent', order: 1.0 },
        { nodeId: 'child-2', order: 2.0 },
        { nodeId: 'child-3', order: 3.0 }
      ];

      const result = calculateOutdentInsertOrderPure(children, 'old-parent');

      // Should be midpoint between 1.0 and 2.0
      expect(result).toBe(1.5);
    });

    it('handles non-integer fractional orders', () => {
      const children = [
        { nodeId: 'old-parent', order: 1.5 },
        { nodeId: 'next', order: 1.75 }
      ];

      const result = calculateOutdentInsertOrderPure(children, 'old-parent');

      // Should be midpoint between 1.5 and 1.75
      expect(result).toBe(1.625);
    });

    it('handles many children with old parent in middle', () => {
      const children = [
        { nodeId: 'child-1', order: 1.0 },
        { nodeId: 'child-2', order: 2.0 },
        { nodeId: 'old-parent', order: 3.0 },
        { nodeId: 'child-4', order: 4.0 },
        { nodeId: 'child-5', order: 5.0 }
      ];

      const result = calculateOutdentInsertOrderPure(children, 'old-parent');

      // Should be midpoint between 3.0 and 4.0
      expect(result).toBe(3.5);
    });
  });

  describe('when old parent is NOT found in children list (fallback)', () => {
    it('appends to end when children exist', () => {
      const children = [
        { nodeId: 'child-1', order: 1.0 },
        { nodeId: 'child-2', order: 2.0 },
        { nodeId: 'child-3', order: 5.0 }
      ];

      const result = calculateOutdentInsertOrderPure(children, 'missing-parent');

      // Should be last order + 1.0 = 5.0 + 1.0 = 6.0
      expect(result).toBe(6.0);
    });

    it('returns 1.0 when parent has no children (empty array)', () => {
      const children: Array<{ nodeId: string; order: number }> = [];

      const result = calculateOutdentInsertOrderPure(children, 'missing-parent');

      // Should be 1.0 (default first position)
      expect(result).toBe(1.0);
    });

    it('handles single child that is not the old parent', () => {
      const children = [{ nodeId: 'other-child', order: 10.0 }];

      const result = calculateOutdentInsertOrderPure(children, 'missing-parent');

      // Should be 10.0 + 1.0 = 11.0
      expect(result).toBe(11.0);
    });
  });

  describe('edge cases', () => {
    it('handles very small order differences (precision)', () => {
      const children = [
        { nodeId: 'old-parent', order: 1.0000001 },
        { nodeId: 'next', order: 1.0000002 }
      ];

      const result = calculateOutdentInsertOrderPure(children, 'old-parent');

      // Should be precise midpoint
      expect(result).toBeCloseTo(1.00000015, 10);
    });

    it('handles very large order values', () => {
      const children = [
        { nodeId: 'old-parent', order: 1000000.0 },
        { nodeId: 'next', order: 2000000.0 }
      ];

      const result = calculateOutdentInsertOrderPure(children, 'old-parent');

      // Should be midpoint
      expect(result).toBe(1500000.0);
    });

    it('handles negative order values', () => {
      const children = [
        { nodeId: 'old-parent', order: -2.0 },
        { nodeId: 'next', order: 0.0 }
      ];

      const result = calculateOutdentInsertOrderPure(children, 'old-parent');

      // Should be midpoint between -2.0 and 0.0
      expect(result).toBe(-1.0);
    });

    it('handles zero order values', () => {
      const children = [
        { nodeId: 'old-parent', order: 0.0 },
        { nodeId: 'next', order: 1.0 }
      ];

      const result = calculateOutdentInsertOrderPure(children, 'old-parent');

      // Should be midpoint between 0.0 and 1.0
      expect(result).toBe(0.5);
    });
  });
});
