/**
 * Tests for pending-operations module
 *
 * Tests the actual exports from $lib/services/pending-operations
 * rather than reimplementing the logic locally.
 *
 * This module tracks pending move operations to prevent race conditions
 * between indent/outdent and content updates.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  waitForPendingMoveOperations,
  trackMoveOperation,
  movesAheadOfWrite,
  type MoveTicket
} from '$lib/services/pending-operations';

/**
 * Generate a unique node ID for testing.
 * Uses random suffix to prevent interference between tests since the
 * pending-operations module uses global state (a Map of pending operations).
 */
const uniqueNodeId = (prefix: string) => `${prefix}-${Math.random().toString(36).slice(2)}`;

/** Track a move that settles when the test says so, exposing its ticket. */
function trackControlledMove(nodeId: string) {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  let ticket!: MoveTicket;
  const tracked = trackMoveOperation(nodeId, (t) => {
    ticket = t;
    return new Promise<void>((res, rej) => {
      resolve = res;
      reject = rej;
    });
  });
  return { tracked, ticket, resolve, reject };
}

describe('pending-operations module', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('trackMoveOperation', () => {
    it('runs the move body synchronously and returns the tracked promise', async () => {
      const nodeId = uniqueNodeId('run-node');
      const ran = vi.fn();
      const tracked = trackMoveOperation(nodeId, async () => {
        ran();
      });

      expect(ran).toHaveBeenCalledTimes(1);
      expect(tracked).toBeInstanceOf(Promise);
      await tracked;
    });

    it('stops tracking a move once it completes', async () => {
      const nodeId = uniqueNodeId('cleanup-node');
      const move = trackControlledMove(nodeId);
      move.ticket.coverWritesThrough(1);
      expect(movesAheadOfWrite(nodeId, 2)).toBeDefined();

      move.resolve();
      await move.tracked;

      expect(movesAheadOfWrite(nodeId, 2)).toBeUndefined();
    });

    it('stops tracking a move even when it fails', async () => {
      const nodeId = uniqueNodeId('fail-node');
      const move = trackControlledMove(nodeId);
      move.ticket.coverWritesThrough(1);

      move.reject(new Error('Move failed'));
      await move.tracked.catch(() => {});

      expect(movesAheadOfWrite(nodeId, 2)).toBeUndefined();
    });

    it("keeps a later move on the same node tracked when an earlier one completes", async () => {
      const nodeId = uniqueNodeId('two-moves-node');
      const first = trackControlledMove(nodeId);
      const second = trackControlledMove(nodeId);
      first.ticket.coverWritesThrough(1);
      second.ticket.coverWritesThrough(2);

      first.resolve();
      await first.tracked;

      expect(movesAheadOfWrite(nodeId, 3)).toBeDefined();
      second.resolve();
      await second.tracked;
      expect(movesAheadOfWrite(nodeId, 3)).toBeUndefined();
    });
  });

  describe('movesAheadOfWrite', () => {
    it('returns undefined for a node with no moves', () => {
      expect(movesAheadOfWrite('non-existent-node-xyz', 1)).toBeUndefined();
    });

    it('does not make a write wait on a move that has not flushed yet', async () => {
      // That move will flush — and so wait on — the write itself.
      const nodeId = uniqueNodeId('unflushed-node');
      const move = trackControlledMove(nodeId);

      expect(movesAheadOfWrite(nodeId, 10)).toBeUndefined();

      move.resolve();
      await move.tracked;
    });

    it('does not make a write wait on a move whose flush covers it', async () => {
      const nodeId = uniqueNodeId('covered-node');
      const move = trackControlledMove(nodeId);
      move.ticket.coverWritesThrough(5);

      expect(movesAheadOfWrite(nodeId, 4)).toBeUndefined();
      expect(movesAheadOfWrite(nodeId, 5)).toBeUndefined();

      move.resolve();
      await move.tracked;
    });

    it('makes a write registered after the flush wait for the move', async () => {
      const nodeId = uniqueNodeId('after-flush-node');
      const move = trackControlledMove(nodeId);
      move.ticket.coverWritesThrough(5);

      const ahead = movesAheadOfWrite(nodeId, 6);
      expect(ahead).toBeDefined();
      const settled = vi.fn();
      void ahead!.then(settled);
      await vi.advanceTimersByTimeAsync(0);
      expect(settled).not.toHaveBeenCalled();

      move.resolve();
      await vi.advanceTimersByTimeAsync(0);
      expect(settled).toHaveBeenCalled();
    });

    it('waits only on the flushed move when a second one is still waiting to flush', async () => {
      // Two quick Tabs, then a checkbox: the write follows the first move and
      // is flushed by the second.
      const nodeId = uniqueNodeId('tab-tab-node');
      const first = trackControlledMove(nodeId);
      first.ticket.coverWritesThrough(3);
      const second = trackControlledMove(nodeId);

      const settled = vi.fn();
      void movesAheadOfWrite(nodeId, 7)!.then(settled);
      first.resolve();
      await vi.advanceTimersByTimeAsync(0);
      expect(settled).toHaveBeenCalled();

      second.resolve();
      await second.tracked;
    });

    it('settles even when the move it waits on fails', async () => {
      const nodeId = uniqueNodeId('failed-ahead-node');
      const move = trackControlledMove(nodeId);
      move.ticket.coverWritesThrough(1);

      const ahead = movesAheadOfWrite(nodeId, 2)!;
      move.reject(new Error('Move failed'));
      await move.tracked.catch(() => {});

      await expect(ahead).resolves.toBeUndefined();
    });
  });

  describe('waitForPendingMoveOperations', () => {
    it('should resolve immediately if no pending operations', async () => {
      const resolved = vi.fn();
      waitForPendingMoveOperations().then(resolved);

      await vi.advanceTimersByTimeAsync(0);
      expect(resolved).toHaveBeenCalled();
    });

    it('waits for every pending move, including two on the same node', async () => {
      const nodeId1 = uniqueNodeId('wait-node-1');
      const nodeId2 = uniqueNodeId('wait-node-2');
      const a = trackControlledMove(nodeId1);
      const b = trackControlledMove(nodeId1);
      const c = trackControlledMove(nodeId2);

      const waitComplete = vi.fn();
      waitForPendingMoveOperations().then(waitComplete);

      a.resolve();
      c.resolve();
      await vi.advanceTimersByTimeAsync(0);
      expect(waitComplete).not.toHaveBeenCalled();

      b.resolve();
      await vi.advanceTimersByTimeAsync(0);
      expect(waitComplete).toHaveBeenCalled();
    });

    it('does not make a move wait on itself', async () => {
      const nodeId = uniqueNodeId('self-node');
      const tracked = trackMoveOperation(nodeId, async () => {
        await waitForPendingMoveOperations();
      });

      const done = vi.fn();
      void tracked.then(done);
      await vi.advanceTimersByTimeAsync(0);
      expect(done).toHaveBeenCalled();
    });
  });

  describe('Race condition prevention (real module)', () => {
    it('should handle the Enter+Tab+Shift+Tab scenario without interleaving indent/outdent operations', async () => {
      vi.useRealTimers();
      const nodeId = uniqueNodeId('indent-outdent-node');
      const operationLog: string[] = [];

      // Fire indent (this is what happens in indentNode)
      const indentPromise = trackMoveOperation(nodeId, async () => {
        await waitForPendingMoveOperations();
        operationLog.push('indent-start');
        await new Promise((r) => setTimeout(r, 100));
        operationLog.push('indent-complete');
      });

      // Fire outdent immediately (simulates rapid Tab then Shift+Tab)
      const outdentPromise = trackMoveOperation(nodeId, async () => {
        await waitForPendingMoveOperations();
        operationLog.push('outdent-start');
        await new Promise((r) => setTimeout(r, 50));
        operationLog.push('outdent-complete');
      });

      await Promise.all([indentPromise, outdentPromise]);

      // Outdent should NOT start until indent completes
      expect(operationLog).toEqual([
        'indent-start',
        'indent-complete',
        'outdent-start',
        'outdent-complete'
      ]);
    });
  });
});
