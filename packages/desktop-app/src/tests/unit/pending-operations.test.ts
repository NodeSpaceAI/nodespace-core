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
  getPendingMoveOperation
} from '$lib/services/pending-operations';

/**
 * Generate a unique node ID for testing.
 * Uses random suffix to prevent interference between tests since the
 * pending-operations module uses global state (a Map of pending operations).
 */
const uniqueNodeId = (prefix: string) => `${prefix}-${Math.random().toString(36).slice(2)}`;

describe('pending-operations module', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('trackMoveOperation', () => {
    it('should track a pending move operation', async () => {
      const nodeId = uniqueNodeId('test-node');
      let resolveOperation: () => void;
      const operation = new Promise<void>((resolve) => {
        resolveOperation = resolve;
      });

      trackMoveOperation(nodeId, operation);

      // Operation should be tracked
      const pending = getPendingMoveOperation(nodeId);
      expect(pending).toBeDefined();

      // Clean up
      resolveOperation!();
      await vi.advanceTimersByTimeAsync(0);
    });

    it('should clean up tracked operations when complete', async () => {
      const nodeId = uniqueNodeId('cleanup-node');
      let resolveOperation: () => void;
      const operation = new Promise<void>((resolve) => {
        resolveOperation = resolve;
      });

      trackMoveOperation(nodeId, operation);
      expect(getPendingMoveOperation(nodeId)).toBeDefined();

      resolveOperation!();
      // Wait for finally() to execute
      await vi.advanceTimersByTimeAsync(0);

      expect(getPendingMoveOperation(nodeId)).toBeUndefined();
    });

    it('should clean up tracked operations even on failure', async () => {
      const nodeId = uniqueNodeId('fail-node');
      let rejectOperation: (error: Error) => void;
      const operation = new Promise<void>((_, reject) => {
        rejectOperation = reject;
      });

      // Track the operation - the tracked promise catches errors internally
      const trackedPromise = trackMoveOperation(nodeId, operation);
      expect(getPendingMoveOperation(nodeId)).toBeDefined();

      rejectOperation!(new Error('Move failed'));

      // The tracked promise should complete (even though the underlying operation failed)
      // We need to catch the error since the tracked promise propagates it
      await trackedPromise.catch(() => {});
      await vi.advanceTimersByTimeAsync(0);

      expect(getPendingMoveOperation(nodeId)).toBeUndefined();
    });

    it('should return the tracked promise', async () => {
      const nodeId = uniqueNodeId('return-node');
      let resolveOperation: () => void;
      const operation = new Promise<void>((resolve) => {
        resolveOperation = resolve;
      });

      const trackedPromise = trackMoveOperation(nodeId, operation);

      // Should return a promise
      expect(trackedPromise).toBeInstanceOf(Promise);

      // Clean up
      resolveOperation!();
      await vi.advanceTimersByTimeAsync(0);
    });
  });

  describe('waitForPendingMoveOperations', () => {
    it('should resolve immediately if no pending operations', async () => {
      // Generate unique node IDs to avoid interference from other tests
      const resolved = vi.fn();
      waitForPendingMoveOperations().then(resolved);

      await vi.advanceTimersByTimeAsync(0);
      expect(resolved).toHaveBeenCalled();
    });

    it('should wait for all pending operations to complete', async () => {
      const completionOrder: string[] = [];
      const nodeId1 = uniqueNodeId('wait-node-1');
      const nodeId2 = uniqueNodeId('wait-node-2');
      let resolveOp1: () => void;
      let resolveOp2: () => void;

      const op1 = new Promise<void>((resolve) => {
        resolveOp1 = () => {
          completionOrder.push('op1');
          resolve();
        };
      });

      const op2 = new Promise<void>((resolve) => {
        resolveOp2 = () => {
          completionOrder.push('op2');
          resolve();
        };
      });

      trackMoveOperation(nodeId1, op1);
      trackMoveOperation(nodeId2, op2);

      const waitComplete = vi.fn();
      waitForPendingMoveOperations().then(waitComplete);

      // Neither operation has completed
      await vi.advanceTimersByTimeAsync(0);
      expect(waitComplete).not.toHaveBeenCalled();

      // Complete first operation
      resolveOp1!();
      await vi.advanceTimersByTimeAsync(0);
      expect(waitComplete).not.toHaveBeenCalled();

      // Complete second operation
      resolveOp2!();
      await vi.advanceTimersByTimeAsync(0);
      expect(waitComplete).toHaveBeenCalled();
    });
  });

  describe('getPendingMoveOperation', () => {
    it('should return undefined for non-tracked nodes', () => {
      const result = getPendingMoveOperation('non-existent-node-xyz');
      expect(result).toBeUndefined();
    });

    it('should return the pending promise for tracked nodes', async () => {
      const nodeId = uniqueNodeId('get-pending-node');
      let resolveOperation: () => void;
      const operation = new Promise<void>((resolve) => {
        resolveOperation = resolve;
      });

      trackMoveOperation(nodeId, operation);

      const pending = getPendingMoveOperation(nodeId);
      expect(pending).toBeDefined();
      expect(pending).toBeInstanceOf(Promise);

      // Clean up
      resolveOperation!();
      await vi.advanceTimersByTimeAsync(0);
    });
  });

  describe('Race condition prevention (real module)', () => {
    // These tests require real timers because:
    // 1. We're testing actual timing coordination between async operations
    // 2. Fake timers don't properly simulate Promise.race/setTimeout interactions
    //    when multiple promises are racing against real delays
    // 3. The module uses real setTimeout internally, which needs real time to test

    it('should ensure subsequent operations wait for pending moves', async () => {
      vi.useRealTimers();
      const nodeId = uniqueNodeId('race-node');
      const executionOrder: string[] = [];

      // First operation takes 50ms
      const op1 = new Promise<void>((resolve) => {
        setTimeout(() => {
          executionOrder.push('op1-complete');
          resolve();
        }, 50);
      });

      // Track first operation
      trackMoveOperation(nodeId, op1);

      // A second operation that waits before starting
      const op2 = (async () => {
        await waitForPendingMoveOperations();
        executionOrder.push('op2-started-after-wait');

        await new Promise<void>((resolve) => {
          setTimeout(() => {
            executionOrder.push('op2-complete');
            resolve();
          }, 10);
        });
      })();

      // Wait for both
      await Promise.all([op1, op2]);

      // op2 should NOT start until op1 completes
      expect(executionOrder[0]).toBe('op1-complete');
      expect(executionOrder[1]).toBe('op2-started-after-wait');
      expect(executionOrder[2]).toBe('op2-complete');
    });

    it('should handle the Enter+Tab+Shift+Tab scenario (Issue #662)', async () => {
      vi.useRealTimers();
      const nodeId = uniqueNodeId('issue-662-node');
      const operationLog: string[] = [];

      // Simulate indent operation (takes 100ms to complete backend call)
      const indentOperation = async () => {
        operationLog.push('indent-start');
        await new Promise((r) => setTimeout(r, 100));
        operationLog.push('indent-complete');
      };

      // Simulate outdent operation that must wait for indent
      const outdentOperation = async () => {
        // This is what outdentNode does - wait for pending moves first
        await waitForPendingMoveOperations();
        operationLog.push('outdent-start');
        await new Promise((r) => setTimeout(r, 50));
        operationLog.push('outdent-complete');
      };

      // Fire indent (this is what happens in indentNode)
      const indentPromise = indentOperation();
      trackMoveOperation(nodeId, indentPromise);

      // Fire outdent immediately (simulates rapid Tab then Shift+Tab)
      const outdentPromise = outdentOperation();

      // Wait for both
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
