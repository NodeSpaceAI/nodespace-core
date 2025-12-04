import { describe, test, expect, beforeEach, vi } from 'vitest';
import { queueDatabaseWrite, getQueueDepth } from '$lib/utils/database-write-queue';

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
});
