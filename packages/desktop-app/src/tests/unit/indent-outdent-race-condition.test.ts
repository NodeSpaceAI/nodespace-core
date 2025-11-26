/**
 * Tests for indent/outdent race condition prevention (Issue #662)
 *
 * Verifies that rapid indent/outdent operations are serialized properly
 * through the pending move operation tracking mechanism.
 *
 * The race condition scenario:
 * 1. User presses Enter to create node B
 * 2. User immediately presses Tab to indent B under A
 * 3. User immediately presses Shift+Tab to outdent B
 * 4. Without proper coordination, outdent may fail with "Sibling not found"
 *    because indent's moveNodeCommand hasn't completed yet
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// Replicate the module-level coordination mechanism for testing
// (These would be internal to reactive-node-service.svelte.ts)
describe('Indent/Outdent Race Condition Prevention (Issue #662)', () => {
  let pendingMoveOperations: Map<string, Promise<void>>;

  async function waitForPendingMoveOperations(): Promise<void> {
    if (pendingMoveOperations.size === 0) return;
    await Promise.all(pendingMoveOperations.values());
  }

  function trackMoveOperation(nodeId: string, operation: Promise<void>): Promise<void> {
    const trackedPromise = operation.finally(() => {
      pendingMoveOperations.delete(nodeId);
    });
    pendingMoveOperations.set(nodeId, trackedPromise);
    return trackedPromise;
  }

  beforeEach(() => {
    pendingMoveOperations = new Map();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('Move operation tracking', () => {
    it('should track pending move operations', () => {
      const operation = new Promise<void>((resolve) => {
        setTimeout(() => resolve(), 100);
      });

      trackMoveOperation('node-1', operation);

      expect(pendingMoveOperations.size).toBe(1);
      expect(pendingMoveOperations.has('node-1')).toBe(true);
    });

    it('should clean up tracked operations when complete', async () => {
      let resolveOperation: () => void;
      const operation = new Promise<void>((resolve) => {
        resolveOperation = resolve;
      });

      trackMoveOperation('node-1', operation);
      expect(pendingMoveOperations.size).toBe(1);

      resolveOperation!();
      // Wait for finally() to execute
      await vi.advanceTimersByTimeAsync(0);

      expect(pendingMoveOperations.size).toBe(0);
    });

    it('should clean up tracked operations even on failure', async () => {
      let rejectOperation: (error: Error) => void;
      const operation = new Promise<void>((_, reject) => {
        rejectOperation = reject;
      });

      // Track the operation - the tracked promise catches errors internally
      const trackedPromise = trackMoveOperation('node-1', operation);
      expect(pendingMoveOperations.size).toBe(1);

      rejectOperation!(new Error('Move failed'));

      // The tracked promise should complete (even though the underlying operation failed)
      // We need to catch the error since the tracked promise propagates it
      await trackedPromise.catch(() => {});
      await vi.advanceTimersByTimeAsync(0);

      expect(pendingMoveOperations.size).toBe(0);
    });
  });

  describe('waitForPendingMoveOperations', () => {
    it('should resolve immediately if no pending operations', async () => {
      const resolved = vi.fn();
      waitForPendingMoveOperations().then(resolved);

      await vi.advanceTimersByTimeAsync(0);
      expect(resolved).toHaveBeenCalled();
    });

    it('should wait for all pending operations to complete', async () => {
      const completionOrder: string[] = [];
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

      trackMoveOperation('node-1', op1);
      trackMoveOperation('node-2', op2);

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

  describe('Race condition prevention', () => {
    it('should ensure subsequent operations wait for pending moves', async () => {
      vi.useRealTimers(); // Use real timers for this test
      const executionOrder: string[] = [];

      // First operation takes 50ms
      const op1 = new Promise<void>((resolve) => {
        setTimeout(() => {
          executionOrder.push('op1-complete');
          resolve();
        }, 50);
      });

      // Track first operation
      trackMoveOperation('node-1', op1);

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

    it('should allow independent operations on different nodes to proceed', async () => {
      vi.useRealTimers();
      const executionOrder: string[] = [];

      const op1 = new Promise<void>((resolve) => {
        setTimeout(() => {
          executionOrder.push('op1');
          resolve();
        }, 50);
      });

      const op2 = new Promise<void>((resolve) => {
        setTimeout(() => {
          executionOrder.push('op2');
          resolve();
        }, 10);
      });

      // Track operations on different nodes
      trackMoveOperation('node-1', op1);
      trackMoveOperation('node-2', op2);

      // Both run concurrently
      await Promise.all([op1, op2]);

      // op2 completes first due to shorter delay
      expect(executionOrder[0]).toBe('op2');
      expect(executionOrder[1]).toBe('op1');
    });

    it('should prevent outdent from starting before indent completes', async () => {
      vi.useRealTimers();
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
      trackMoveOperation('node-1', indentPromise);

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

    it('should handle rapid Enter+Tab+Shift+Tab sequence (Issue #662 scenario)', async () => {
      vi.useRealTimers();
      const operationLog: string[] = [];
      let indentResolve: () => void;

      // Mock node B creation from Enter key (debounced save)
      const createNodeB = async () => {
        operationLog.push('create-B');
        await new Promise((r) => setTimeout(r, 20));
        operationLog.push('create-B-saved');
      };

      // Mock Tab key indent (B becomes child of A)
      const indentB = async () => {
        operationLog.push('indent-B-start');
        // This takes time - backend needs to create edge A->B
        await new Promise<void>((resolve) => {
          indentResolve = () => {
            operationLog.push('indent-B-complete');
            resolve();
          };
          setTimeout(indentResolve, 100);
        });
      };

      // Mock Shift+Tab outdent (B becomes sibling of A)
      const outdentB = async () => {
        // Must wait for indent to complete first!
        await waitForPendingMoveOperations();
        operationLog.push('outdent-B-start');
        await new Promise((r) => setTimeout(r, 30));
        operationLog.push('outdent-B-complete');
      };

      // Simulate rapid sequence
      await createNodeB();

      // User immediately presses Tab
      const indentPromise = indentB();
      trackMoveOperation('node-B', indentPromise);

      // User immediately presses Shift+Tab
      const outdentPromise = outdentB();

      // Wait for both operations
      await Promise.all([indentPromise, outdentPromise]);

      // Verify correct order: indent must complete before outdent starts
      const indentCompleteIndex = operationLog.indexOf('indent-B-complete');
      const outdentStartIndex = operationLog.indexOf('outdent-B-start');

      expect(indentCompleteIndex).toBeLessThan(outdentStartIndex);
      expect(operationLog).toEqual([
        'create-B',
        'create-B-saved',
        'indent-B-start',
        'indent-B-complete',
        'outdent-B-start',
        'outdent-B-complete'
      ]);
    });
  });
});
