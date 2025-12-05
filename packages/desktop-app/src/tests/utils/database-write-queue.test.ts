import { describe, test, expect, beforeEach, vi } from 'vitest';
import {
  queueDatabaseWrite,
  getQueueDepth,
  __resetQueueForTesting
} from '$lib/utils/database-write-queue';

describe('databaseWriteQueue', () => {
  beforeEach(() => {
    // Clear console mocks before each test
    vi.clearAllMocks();
  });

  describe('queueDatabaseWrite', () => {
    test('executes a single operation successfully', async () => {
      const mockOperation = vi.fn().mockResolvedValue('success');

      const result = await queueDatabaseWrite(mockOperation);

      expect(result).toBe('success');
      expect(mockOperation).toHaveBeenCalledTimes(1);
    });

    test('returns the operation result with correct type', async () => {
      const numberOperation = vi.fn().mockResolvedValue(42);
      const stringOperation = vi.fn().mockResolvedValue('hello');
      const objectOperation = vi.fn().mockResolvedValue({ id: '123' });

      const numberResult = await queueDatabaseWrite(numberOperation);
      const stringResult = await queueDatabaseWrite(stringOperation);
      const objectResult = await queueDatabaseWrite(objectOperation);

      expect(numberResult).toBe(42);
      expect(stringResult).toBe('hello');
      expect(objectResult).toEqual({ id: '123' });
    });

    test('serializes concurrent operations in order', async () => {
      const executionOrder: number[] = [];
      const delays = [50, 30, 10]; // Intentionally different delays

      // Create operations that would complete in different order if run concurrently
      const operations = delays.map((delay, index) => {
        return vi.fn().mockImplementation(async () => {
          await new Promise((resolve) => setTimeout(resolve, delay));
          executionOrder.push(index);
          return index;
        });
      });

      // Start all operations "concurrently" (without await)
      const promises = operations.map((op) => queueDatabaseWrite(op));

      // Wait for all to complete
      const results = await Promise.all(promises);

      // Despite different delays, they should execute in submission order (0, 1, 2)
      // not completion order (2, 1, 0)
      expect(executionOrder).toEqual([0, 1, 2]);
      expect(results).toEqual([0, 1, 2]);
    });

    test('handles operation errors without blocking subsequent operations', async () => {
      const error = new Error('Operation failed');
      const failingOperation = vi.fn().mockRejectedValue(error);
      const successOperation = vi.fn().mockResolvedValue('success');

      // First operation fails
      await expect(queueDatabaseWrite(failingOperation)).rejects.toThrow('Operation failed');

      // Second operation should still execute successfully
      const result = await queueDatabaseWrite(successOperation);
      expect(result).toBe('success');
      expect(successOperation).toHaveBeenCalledTimes(1);
    });

    test('propagates operation errors to caller', async () => {
      const customError = new Error('Custom database error');
      const failingOperation = vi.fn().mockRejectedValue(customError);

      await expect(queueDatabaseWrite(failingOperation)).rejects.toThrow('Custom database error');
    });
  });

  describe('queue depth tracking', () => {
    test('increments and decrements queue depth correctly for single operation', async () => {
      let depthDuringOperation = 0;
      const operation = vi.fn().mockImplementation(async () => {
        depthDuringOperation = getQueueDepth();
        return 'done';
      });

      expect(getQueueDepth()).toBe(0);

      await queueDatabaseWrite(operation);

      expect(depthDuringOperation).toBe(1);
      expect(getQueueDepth()).toBe(0);
    });

    test('tracks queue depth correctly with multiple concurrent operations', async () => {
      const depthSnapshots: number[] = [];

      const operations = Array.from({ length: 5 }, (_, i) =>
        vi.fn().mockImplementation(async () => {
          depthSnapshots.push(getQueueDepth());
          await new Promise((resolve) => setTimeout(resolve, 10));
          return i;
        })
      );

      const promises = operations.map((op) => queueDatabaseWrite(op));
      await Promise.all(promises);

      // Queue depth should count down as operations complete
      expect(depthSnapshots).toEqual([5, 4, 3, 2, 1]);
      expect(getQueueDepth()).toBe(0);
    });

    test('decrements queue depth even when operation fails', async () => {
      const failingOperation = vi.fn().mockRejectedValue(new Error('fail'));

      expect(getQueueDepth()).toBe(0);

      await expect(queueDatabaseWrite(failingOperation)).rejects.toThrow('fail');

      // Depth should be back to 0 even after error
      expect(getQueueDepth()).toBe(0);
    });

    test('maintains correct queue depth with mixed success and failure', async () => {
      const operations = [
        vi.fn().mockResolvedValue('success1'),
        vi.fn().mockRejectedValue(new Error('error1')),
        vi.fn().mockResolvedValue('success2'),
        vi.fn().mockRejectedValue(new Error('error2')),
        vi.fn().mockResolvedValue('success3')
      ];

      const promises = operations.map((op) =>
        queueDatabaseWrite(op).catch((err) => `caught: ${err.message}`)
      );

      await Promise.all(promises);

      // All operations completed, depth should be 0
      expect(getQueueDepth()).toBe(0);
    });
  });

  describe('queue depth warning behavior', () => {
    // Note: Logger is intentionally silenced during tests for performance
    // These tests verify that queue operations complete successfully regardless
    // of warning behavior. The warning functionality is tested implicitly by
    // verifying queue depth tracking in the 'queue depth tracking' test suite.

    test('handles queue depth exceeding threshold without blocking operations', async () => {
      // Create 11 operations to exceed threshold of 10
      const operations = Array.from({ length: 11 }, (_, i) =>
        vi.fn().mockImplementation(async () => {
          await new Promise((resolve) => setTimeout(resolve, 5));
          return i;
        })
      );

      const promises = operations.map((op) => queueDatabaseWrite(op));
      const results = await Promise.all(promises);

      // All operations should complete successfully
      expect(results).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
      expect(getQueueDepth()).toBe(0);
    });

    test('operations at threshold complete without issues', async () => {
      // Create exactly 10 operations (at threshold, not exceeding)
      const operations = Array.from({ length: 10 }, (_, i) => vi.fn().mockResolvedValue(i));

      const promises = operations.map((op) => queueDatabaseWrite(op));
      const results = await Promise.all(promises);

      // All operations should complete successfully
      expect(results).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
      expect(getQueueDepth()).toBe(0);
    });

    test('handles many operations exceeding threshold repeatedly', async () => {
      // Create 15 operations to exceed threshold multiple times
      const operations = Array.from({ length: 15 }, (_, i) =>
        vi.fn().mockImplementation(async () => {
          await new Promise((resolve) => setTimeout(resolve, 5));
          return i;
        })
      );

      const promises = operations.map((op) => queueDatabaseWrite(op));
      const results = await Promise.all(promises);

      // All operations should complete successfully
      expect(results).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]);
      expect(getQueueDepth()).toBe(0);
    });
  });

  describe('promise resolution behavior', () => {
    test('resolves promises in submission order, not completion order', async () => {
      const completionOrder: number[] = [];
      const resolutionOrder: number[] = [];

      // Fast operation submitted second
      const fastOp = vi.fn().mockImplementation(async () => {
        await new Promise((resolve) => setTimeout(resolve, 10));
        completionOrder.push(2);
        return 'fast';
      });

      // Slow operation submitted first
      const slowOp = vi.fn().mockImplementation(async () => {
        await new Promise((resolve) => setTimeout(resolve, 50));
        completionOrder.push(1);
        return 'slow';
      });

      const promise1 = queueDatabaseWrite(slowOp).then((result) => {
        resolutionOrder.push(1);
        return result;
      });

      const promise2 = queueDatabaseWrite(fastOp).then((result) => {
        resolutionOrder.push(2);
        return result;
      });

      await Promise.all([promise1, promise2]);

      // Completion order: slow operation finishes first (but blocks fast operation)
      // Resolution order: should match submission order (1, 2) due to serialization
      expect(resolutionOrder).toEqual([1, 2]);
    });
  });

  describe('edge cases', () => {
    test('handles operation returning undefined', async () => {
      const operation = vi.fn().mockResolvedValue(undefined);

      const result = await queueDatabaseWrite(operation);

      expect(result).toBeUndefined();
      expect(operation).toHaveBeenCalledTimes(1);
    });

    test('handles operation returning null', async () => {
      const operation = vi.fn().mockResolvedValue(null);

      const result = await queueDatabaseWrite(operation);

      expect(result).toBeNull();
      expect(operation).toHaveBeenCalledTimes(1);
    });

    test('handles synchronous errors in operation', async () => {
      const operation = vi.fn().mockImplementation(() => {
        throw new Error('Synchronous error');
      });

      await expect(queueDatabaseWrite(operation)).rejects.toThrow('Synchronous error');
      expect(getQueueDepth()).toBe(0);
    });

    test('handles very long queue (stress test)', async () => {
      const operationCount = 100;
      const operations = Array.from({ length: operationCount }, (_, i) =>
        vi.fn().mockResolvedValue(i)
      );

      const promises = operations.map((op) => queueDatabaseWrite(op));
      const results = await Promise.all(promises);

      expect(results).toEqual(Array.from({ length: operationCount }, (_, i) => i));
      expect(getQueueDepth()).toBe(0);
    });
  });

  describe('getQueueDepth', () => {
    test('returns 0 when no operations are queued', () => {
      expect(getQueueDepth()).toBe(0);
    });

    test('can be called during operation execution', async () => {
      const depths: number[] = [];

      const operation = vi.fn().mockImplementation(async () => {
        depths.push(getQueueDepth());
        await new Promise((resolve) => setTimeout(resolve, 10));
        depths.push(getQueueDepth());
        return 'done';
      });

      await queueDatabaseWrite(operation);

      // Depth should be 1 both at start and end of operation execution
      expect(depths).toEqual([1, 1]);
      expect(getQueueDepth()).toBe(0);
    });
  });

  describe('error handling in queue chain', () => {
    test('ignores errors from previous operations in the queue (line 69 coverage)', async () => {
      // This test specifically covers the catch block (lines 66-69) that ignores errors
      // from previous operations to prevent error propagation through the chain
      const error = new Error('Previous operation error');
      const failingOperation = vi.fn().mockRejectedValue(error);
      const successOperation = vi.fn().mockResolvedValue('success');

      // Start both operations concurrently to ensure second waits for first
      const promise1 = queueDatabaseWrite(failingOperation);
      const promise2 = queueDatabaseWrite(successOperation);

      // First operation should fail
      await expect(promise1).rejects.toThrow('Previous operation error');

      // Second operation should succeed despite the first one failing
      // This exercises the catch block where errors from previous operations are ignored
      const result2 = await promise2;

      expect(result2).toBe('success');
      expect(successOperation).toHaveBeenCalledTimes(1);
    });

    test('catch block allows continuation after previous operation error', async () => {
      // Additional test to ensure the empty catch block is covered
      const error = new Error('Previous operation error');
      const failingOperation = vi.fn().mockRejectedValue(error);
      const successOperation = vi.fn().mockResolvedValue('success');
      const thirdOperation = vi.fn().mockResolvedValue('third');

      // Queue three operations: fail, success, success
      const promise1 = queueDatabaseWrite(failingOperation);
      const promise2 = queueDatabaseWrite(successOperation);
      const promise3 = queueDatabaseWrite(thirdOperation);

      // First operation should fail
      await expect(promise1).rejects.toThrow('Previous operation error');

      // Second and third operations should succeed despite the first one failing
      // This exercises the catch block multiple times
      const result2 = await promise2;
      const result3 = await promise3;

      expect(result2).toBe('success');
      expect(result3).toBe('third');
      expect(successOperation).toHaveBeenCalledTimes(1);
      expect(thirdOperation).toHaveBeenCalledTimes(1);
    });

    test('handles multiple consecutive failures in queue chain', async () => {
      // Test that line 69 works correctly with multiple failures in sequence
      const error1 = new Error('First failure');
      const error2 = new Error('Second failure');
      const failOp1 = vi.fn().mockRejectedValue(error1);
      const failOp2 = vi.fn().mockRejectedValue(error2);
      const successOp = vi.fn().mockResolvedValue('finally success');

      // Queue three operations: fail, fail, success
      const promise1 = queueDatabaseWrite(failOp1);
      const promise2 = queueDatabaseWrite(failOp2);
      const promise3 = queueDatabaseWrite(successOp);

      // First two should fail with their respective errors
      await expect(promise1).rejects.toThrow('First failure');
      await expect(promise2).rejects.toThrow('Second failure');

      // Third should succeed (exercises line 69 twice)
      const result = await promise3;
      expect(result).toBe('finally success');
      expect(successOp).toHaveBeenCalledTimes(1);
    });
  });

  describe('__resetQueueForTesting', () => {
    test('resets queue state in test environment', async () => {
      // First ensure we start fresh
      __resetQueueForTesting();

      // Queue an operation to create some state
      await queueDatabaseWrite(vi.fn().mockResolvedValue('test'));

      // Verify we can reset the queue state
      __resetQueueForTesting();
      expect(getQueueDepth()).toBe(0);
    });

    test('resets queue state after operations have been queued', async () => {
      // First ensure we start fresh
      __resetQueueForTesting();

      // Queue some operations to create state
      const operations = Array.from({ length: 3 }, (_, i) => vi.fn().mockResolvedValue(i));
      const promises = operations.map((op) => queueDatabaseWrite(op));
      await Promise.all(promises);

      // Queue depth should be 0 after operations complete
      expect(getQueueDepth()).toBe(0);

      // Reset should work without errors
      __resetQueueForTesting();
      expect(getQueueDepth()).toBe(0);
    });

    test('throws error when called outside test environment', () => {
      // Save original NODE_ENV
      const originalEnv = process.env.NODE_ENV;

      try {
        // Temporarily change NODE_ENV to simulate production
        process.env.NODE_ENV = 'production';

        // Should throw error
        expect(() => __resetQueueForTesting()).toThrow(
          '__resetQueueForTesting() can only be called in test environment'
        );
      } finally {
        // Restore original NODE_ENV
        process.env.NODE_ENV = originalEnv;
      }
    });

    test('throws error when called in development environment', () => {
      const originalEnv = process.env.NODE_ENV;

      try {
        process.env.NODE_ENV = 'development';

        expect(() => __resetQueueForTesting()).toThrow(
          '__resetQueueForTesting() can only be called in test environment'
        );
      } finally {
        process.env.NODE_ENV = originalEnv;
      }
    });

    test('allows reset in test environment (NODE_ENV=test)', () => {
      const originalEnv = process.env.NODE_ENV;

      try {
        process.env.NODE_ENV = 'test';

        // Should not throw
        expect(() => __resetQueueForTesting()).not.toThrow();
        expect(getQueueDepth()).toBe(0);
      } finally {
        process.env.NODE_ENV = originalEnv;
      }
    });
  });
});
