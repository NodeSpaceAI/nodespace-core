/**
 * PersistenceCoordinator Tests
 *
 * Comprehensive test suite for the declarative dependency-based persistence system.
 * Tests cover:
 * - Basic persistence operations (immediate and debounced)
 * - Dependency resolution (simple, lambda, batch, handle)
 * - Concurrent operation handling
 * - Error handling and recovery
 * - Test mode functionality
 * - Performance metrics
 * - Edge cases and race conditions
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { PersistenceCoordinator } from '$lib/services/persistence-coordinator.svelte';

describe('PersistenceCoordinator', () => {
  let coordinator: PersistenceCoordinator;

  beforeEach(() => {
    // Get fresh instance for each test
    PersistenceCoordinator.resetInstance();
    coordinator = PersistenceCoordinator.getInstance();
    coordinator.enableTestMode();
    coordinator.resetTestState();
  });

  afterEach(async () => {
    await coordinator.reset();
  });

  // ========================================================================
  // Singleton Pattern
  // ========================================================================

  describe('Singleton Pattern', () => {
    it('should return the same instance', () => {
      const instance1 = PersistenceCoordinator.getInstance();
      const instance2 = PersistenceCoordinator.getInstance();

      expect(instance1).toBe(instance2);
    });

    it('should reset instance for testing', () => {
      const instance1 = PersistenceCoordinator.getInstance();
      PersistenceCoordinator.resetInstance();
      const instance2 = PersistenceCoordinator.getInstance();

      expect(instance1).not.toBe(instance2);
    });
  });

  // ========================================================================
  // Basic Persistence Operations
  // ========================================================================

  describe('Basic Persistence', () => {
    it('should persist a node immediately', async () => {
      const nodeId = 'test-node-1';
      let executed = false;

      const handle = coordinator.persist(
        nodeId,
        async () => {
          executed = true;
        },
        { mode: 'immediate' }
      );

      await handle.promise;

      expect(executed).toBe(true);
      expect(coordinator.isPersisted(nodeId)).toBe(true);
      expect(coordinator.isPending(nodeId)).toBe(false);
      expect(coordinator.getStatus(nodeId)).toBe('completed');
    });

    it('should persist a node with debouncing', async () => {
      const nodeId = 'test-node-2';
      let executed = false;

      const handle = coordinator.persist(
        nodeId,
        async () => {
          executed = true;
        },
        { mode: 'debounce', debounceMs: 50 }
      );

      // Should not execute immediately
      expect(executed).toBe(false);
      expect(coordinator.isPending(nodeId)).toBe(true);

      // Wait for debounce
      await handle.promise;

      expect(executed).toBe(true);
      expect(coordinator.isPersisted(nodeId)).toBe(true);
    });

    it('should cancel pending debounced operation when new one requested', async () => {
      const nodeId = 'test-node-3';
      let firstExecuted = false;
      let secondExecuted = false;

      // First operation
      const handle1 = coordinator.persist(
        nodeId,
        async () => {
          firstExecuted = true;
        },
        { mode: 'debounce', debounceMs: 100 }
      );

      // Catch expected cancellation error
      handle1.promise.catch(() => {
        /* Expected - operation will be cancelled */
      });

      // Second operation (should cancel first)
      await new Promise((resolve) => setTimeout(resolve, 20));
      const handle2 = coordinator.persist(
        nodeId,
        async () => {
          secondExecuted = true;
        },
        { mode: 'debounce', debounceMs: 50 }
      );

      await handle2.promise;

      expect(firstExecuted).toBe(false);
      expect(secondExecuted).toBe(true);
    });
  });

  // ========================================================================
  // Dependency Resolution
  // ========================================================================

  describe('Dependency Resolution', () => {
    it('should resolve simple node ID dependency', async () => {
      const executionOrder: string[] = [];

      // Parent operation
      const parentHandle = coordinator.persist(
        'parent',
        async () => {
          executionOrder.push('parent');
          await new Promise((resolve) => setTimeout(resolve, 20));
        },
        { mode: 'immediate' }
      );

      // Child operation depends on parent
      const childHandle = coordinator.persist(
        'child',
        async () => {
          executionOrder.push('child');
        },
        {
          mode: 'immediate',
          dependencies: ['parent']
        }
      );

      await Promise.all([parentHandle.promise, childHandle.promise]);

      expect(executionOrder).toEqual(['parent', 'child']);
    });

    it('should resolve lambda dependency', async () => {
      const executionOrder: string[] = [];
      let lambdaExecuted = false;

      const handle = coordinator.persist(
        'node',
        async () => {
          executionOrder.push('node');
        },
        {
          mode: 'immediate',
          dependencies: [
            async () => {
              executionOrder.push('lambda');
              lambdaExecuted = true;
              await new Promise((resolve) => setTimeout(resolve, 10));
            }
          ]
        }
      );

      await handle.promise;

      expect(lambdaExecuted).toBe(true);
      expect(executionOrder).toEqual(['lambda', 'node']);
    });

    it('should resolve batch dependency', async () => {
      const executionOrder: string[] = [];

      // Create multiple parent nodes
      const parent1 = coordinator.persist(
        'parent1',
        async () => {
          executionOrder.push('parent1');
          await new Promise((resolve) => setTimeout(resolve, 10));
        },
        { mode: 'immediate' }
      );

      const parent2 = coordinator.persist(
        'parent2',
        async () => {
          executionOrder.push('parent2');
          await new Promise((resolve) => setTimeout(resolve, 10));
        },
        { mode: 'immediate' }
      );

      // Child depends on both parents
      const child = coordinator.persist(
        'child',
        async () => {
          executionOrder.push('child');
        },
        {
          mode: 'immediate',
          dependencies: [{ nodeIds: ['parent1', 'parent2'] }]
        }
      );

      await Promise.all([parent1.promise, parent2.promise, child.promise]);

      // Child should execute after both parents
      expect(executionOrder.indexOf('child')).toBeGreaterThan(executionOrder.indexOf('parent1'));
      expect(executionOrder.indexOf('child')).toBeGreaterThan(executionOrder.indexOf('parent2'));
    });

    it('should resolve handle dependency', async () => {
      const executionOrder: string[] = [];

      const parentHandle = coordinator.persist(
        'parent',
        async () => {
          executionOrder.push('parent');
          await new Promise((resolve) => setTimeout(resolve, 10));
        },
        { mode: 'immediate' }
      );

      const childHandle = coordinator.persist(
        'child',
        async () => {
          executionOrder.push('child');
        },
        {
          mode: 'immediate',
          dependencies: [parentHandle]
        }
      );

      await Promise.all([parentHandle.promise, childHandle.promise]);

      expect(executionOrder).toEqual(['parent', 'child']);
    });

    it('should resolve complex multi-level dependencies', async () => {
      const executionOrder: string[] = [];

      // Grandparent
      const grandparent = coordinator.persist(
        'grandparent',
        async () => {
          executionOrder.push('grandparent');
          await new Promise((resolve) => setTimeout(resolve, 10));
        },
        { mode: 'immediate' }
      );

      // Parent depends on grandparent
      const parent = coordinator.persist(
        'parent',
        async () => {
          executionOrder.push('parent');
          await new Promise((resolve) => setTimeout(resolve, 10));
        },
        {
          mode: 'immediate',
          dependencies: ['grandparent']
        }
      );

      // Child depends on parent
      const child = coordinator.persist(
        'child',
        async () => {
          executionOrder.push('child');
        },
        {
          mode: 'immediate',
          dependencies: ['parent']
        }
      );

      await Promise.all([grandparent.promise, parent.promise, child.promise]);

      expect(executionOrder).toEqual(['grandparent', 'parent', 'child']);
    });

    it('should handle missing dependencies gracefully', async () => {
      let executed = false;

      // Depend on non-existent node (should not block)
      const handle = coordinator.persist(
        'node',
        async () => {
          executed = true;
        },
        {
          mode: 'immediate',
          dependencies: ['non-existent-node']
        }
      );

      await handle.promise;

      expect(executed).toBe(true);
    });
  });

  // ========================================================================
  // Concurrent Operations
  // ========================================================================

  describe('Concurrent Operations', () => {
    it('should handle multiple concurrent operations', async () => {
      const executionCounts = new Map<string, number>();

      const operations = Array.from({ length: 10 }, (_, i) => {
        const nodeId = `node-${i}`;
        return coordinator.persist(
          nodeId,
          async () => {
            executionCounts.set(nodeId, (executionCounts.get(nodeId) || 0) + 1);
            await new Promise((resolve) => setTimeout(resolve, 5));
          },
          { mode: 'immediate' }
        );
      });

      await Promise.all(operations.map((op) => op.promise));

      // Each operation should execute exactly once
      expect(executionCounts.size).toBe(10);
      for (const count of executionCounts.values()) {
        expect(count).toBe(1);
      }
    });

    it('should handle rapid updates to same node (debouncing)', async () => {
      const nodeId = 'rapid-update-node';
      let executionCount = 0;

      // Rapid updates
      const handles = Array.from({ length: 10 }, () => {
        return coordinator.persist(
          nodeId,
          async () => {
            executionCount++;
          },
          { mode: 'debounce', debounceMs: 50 }
        );
      });

      // Catch expected cancellation errors for all but the last
      for (let i = 0; i < handles.length - 1; i++) {
        handles[i].promise.catch(() => {
          /* Expected - operation will be cancelled */
        });
      }

      // Only the last operation should execute
      await handles[handles.length - 1].promise;

      expect(executionCount).toBe(1);
    });
  });

  // ========================================================================
  // Error Handling
  // ========================================================================

  describe('Error Handling', () => {
    it('should handle operation errors', async () => {
      const nodeId = 'error-node';
      const error = new Error('Test error');

      const handle = coordinator.persist(
        nodeId,
        async () => {
          throw error;
        },
        { mode: 'immediate' }
      );

      await expect(handle.promise).rejects.toThrow('Test error');
      expect(coordinator.getStatus(nodeId)).toBe('failed');
      expect(coordinator.isPersisted(nodeId)).toBe(false);
    });

    it('should update metrics on failure', async () => {
      const metricsBeforeError = coordinator.getMetrics();

      const handle = coordinator.persist(
        'fail-node',
        async () => {
          throw new Error('Test error');
        },
        { mode: 'immediate' }
      );

      await expect(handle.promise).rejects.toThrow();

      const metricsAfterError = coordinator.getMetrics();
      expect(metricsAfterError.failedOperations).toBe(metricsBeforeError.failedOperations + 1);
    });

    it('should not affect other operations when one fails', async () => {
      const successNodeId = 'success-node';
      const failNodeId = 'fail-node';
      let successExecuted = false;

      const successHandle = coordinator.persist(
        successNodeId,
        async () => {
          successExecuted = true;
        },
        { mode: 'immediate' }
      );

      const failHandle = coordinator.persist(
        failNodeId,
        async () => {
          throw new Error('Test error');
        },
        { mode: 'immediate' }
      );

      await successHandle.promise;
      await expect(failHandle.promise).rejects.toThrow();

      expect(successExecuted).toBe(true);
      expect(coordinator.isPersisted(successNodeId)).toBe(true);
      expect(coordinator.isPersisted(failNodeId)).toBe(false);
    });
  });

  // ========================================================================
  // Wait for Persistence
  // ========================================================================

  describe('Wait for Persistence', () => {
    it('should wait for multiple nodes to persist', async () => {
      const nodes = ['node1', 'node2', 'node3'];

      // Start all operations
      nodes.forEach((nodeId) => {
        coordinator.persist(
          nodeId,
          async () => {
            await new Promise((resolve) => setTimeout(resolve, 20));
          },
          { mode: 'immediate' }
        );
      });

      const failed = await coordinator.waitForPersistence(nodes, 1000);

      expect(failed.size).toBe(0);
      nodes.forEach((nodeId) => {
        expect(coordinator.isPersisted(nodeId)).toBe(true);
      });
    });

    it('should timeout if operations take too long', async () => {
      const nodeId = 'slow-node';

      coordinator.persist(
        nodeId,
        async () => {
          await new Promise((resolve) => setTimeout(resolve, 200));
        },
        { mode: 'immediate' }
      );

      const failed = await coordinator.waitForPersistence([nodeId], 50);

      expect(failed.has(nodeId)).toBe(true);
    });

    it('should return empty set if no pending operations', async () => {
      const failed = await coordinator.waitForPersistence(['non-existent'], 100);

      expect(failed.size).toBe(0);
    });
  });

  // ========================================================================
  // Cancel Operations
  // ========================================================================

  describe('Cancel Operations', () => {
    it('should cancel pending debounced operation', async () => {
      const nodeId = 'cancel-node';
      let executed = false;

      const handle = coordinator.persist(
        nodeId,
        async () => {
          executed = true;
        },
        { mode: 'debounce', debounceMs: 100 }
      );

      // Catch expected cancellation error
      handle.promise.catch(() => {
        /* Expected - operation will be cancelled */
      });

      // Cancel before debounce completes
      coordinator.cancelPending(nodeId);

      await new Promise((resolve) => setTimeout(resolve, 150));

      expect(executed).toBe(false);
      expect(coordinator.getStatus(nodeId)).toBe('failed');
    });

    it('should not cancel in-progress operation', async () => {
      const nodeId = 'in-progress-node';
      let executed = false;

      const handle = coordinator.persist(
        nodeId,
        async () => {
          await new Promise((resolve) => setTimeout(resolve, 50));
          executed = true;
        },
        { mode: 'immediate' }
      );

      // Try to cancel after operation starts
      await new Promise((resolve) => setTimeout(resolve, 10));
      coordinator.cancelPending(nodeId);

      await handle.promise;

      expect(executed).toBe(true);
    });
  });

  // ========================================================================
  // Test Mode
  // ========================================================================

  describe('Test Mode', () => {
    it('should track mock persistence in test mode', async () => {
      coordinator.enableTestMode();

      const nodeId = 'test-mode-node';
      let operationExecuted = false;

      const handle = coordinator.persist(
        nodeId,
        async () => {
          // This SHOULD execute even in test mode (user callback always runs)
          operationExecuted = true;
        },
        { mode: 'immediate' }
      );

      await handle.promise;

      expect(operationExecuted).toBe(true);
      expect(coordinator.isPersisted(nodeId)).toBe(true);
      expect(coordinator.getMockPersistedNodes()).toContain(nodeId);
    });

    it('should disable test mode', async () => {
      coordinator.disableTestMode();

      const nodeId = 'real-mode-node';
      let realOperationExecuted = false;

      const handle = coordinator.persist(
        nodeId,
        async () => {
          realOperationExecuted = true;
        },
        { mode: 'immediate' }
      );

      await handle.promise;

      expect(realOperationExecuted).toBe(true);
    });

    it('should reset test state', async () => {
      const nodeId = 'reset-test-node';

      await coordinator.persist(nodeId, async () => {}, { mode: 'immediate' }).promise;

      expect(coordinator.getMockPersistedNodes()).toContain(nodeId);

      coordinator.resetTestState();

      expect(coordinator.getMockPersistedNodes()).toHaveLength(0);
      expect(coordinator.isPersisted(nodeId)).toBe(false);
    });
  });

  // ========================================================================
  // Metrics
  // ========================================================================

  describe('Performance Metrics', () => {
    it('should track total operations', async () => {
      const metricsBefore = coordinator.getMetrics();

      await coordinator.persist('metrics-node-1', async () => {}, { mode: 'immediate' }).promise;

      await coordinator.persist('metrics-node-2', async () => {}, { mode: 'immediate' }).promise;

      const metricsAfter = coordinator.getMetrics();

      expect(metricsAfter.totalOperations).toBe(metricsBefore.totalOperations + 2);
    });

    it('should track completed operations', async () => {
      const metricsBefore = coordinator.getMetrics();

      await coordinator.persist('completed-node', async () => {}, { mode: 'immediate' }).promise;

      const metricsAfter = coordinator.getMetrics();

      expect(metricsAfter.completedOperations).toBe(metricsBefore.completedOperations + 1);
    });

    it('should track failed operations', async () => {
      const metricsBefore = coordinator.getMetrics();

      const handle = coordinator.persist(
        'failed-node',
        async () => {
          throw new Error('Test error');
        },
        { mode: 'immediate' }
      );

      await expect(handle.promise).rejects.toThrow();

      const metricsAfter = coordinator.getMetrics();

      expect(metricsAfter.failedOperations).toBe(metricsBefore.failedOperations + 1);
    });

    it('should track execution time', async () => {
      await coordinator.persist(
        'timed-node',
        async () => {
          await new Promise((resolve) => setTimeout(resolve, 10));
        },
        { mode: 'immediate' }
      ).promise;

      const metricsAfter = coordinator.getMetrics();

      expect(metricsAfter.averageExecutionTime).toBeGreaterThan(0);
      expect(metricsAfter.maxExecutionTime).toBeGreaterThan(0);
    });

    it('should track pending operations', async () => {
      const nodeId = 'pending-metrics-node';

      coordinator.persist(
        nodeId,
        async () => {
          await new Promise((resolve) => setTimeout(resolve, 50));
        },
        { mode: 'immediate' }
      );

      // Check pending count increases
      await new Promise((resolve) => setTimeout(resolve, 10));
      const metricsDuring = coordinator.getMetrics();
      expect(metricsDuring.pendingOperations).toBeGreaterThan(0);
    });
  });

  // ========================================================================
  // Reactive State
  // ========================================================================

  describe('Reactive State', () => {
    it('should provide reactive isPersisted check', async () => {
      const nodeId = 'reactive-node';

      expect(coordinator.isPersisted(nodeId)).toBe(false);

      await coordinator.persist(nodeId, async () => {}, { mode: 'immediate' }).promise;

      expect(coordinator.isPersisted(nodeId)).toBe(true);
    });

    it('should provide reactive isPending check', async () => {
      const nodeId = 'pending-reactive-node';

      expect(coordinator.isPending(nodeId)).toBe(false);

      const handle = coordinator.persist(
        nodeId,
        async () => {
          await new Promise((resolve) => setTimeout(resolve, 20));
        },
        { mode: 'immediate' }
      );

      // Should be pending during execution
      expect(coordinator.isPending(nodeId)).toBe(true);

      await handle.promise;

      // Should not be pending after completion
      expect(coordinator.isPending(nodeId)).toBe(false);
    });

    it('should provide reactive status check', async () => {
      const nodeId = 'status-reactive-node';

      expect(coordinator.getStatus(nodeId)).toBeUndefined();

      const handle = coordinator.persist(
        nodeId,
        async () => {
          await new Promise((resolve) => setTimeout(resolve, 20));
        },
        { mode: 'immediate' }
      );

      // Status should be pending initially
      expect(coordinator.getStatus(nodeId)).toBe('pending');

      // Wait a bit for it to start
      await new Promise((resolve) => setTimeout(resolve, 5));
      expect(coordinator.getStatus(nodeId)).toBe('in-progress');

      await handle.promise;

      // Status should be completed after promise resolves
      expect(coordinator.getStatus(nodeId)).toBe('completed');
    });
  });

  // ========================================================================
  // Reset Functionality
  // ========================================================================

  describe('Reset Functionality', () => {
    it('should reset all state', async () => {
      // Create some operations
      await coordinator.persist('node1', async () => {}, { mode: 'immediate' }).promise;
      await coordinator.persist('node2', async () => {}, { mode: 'immediate' }).promise;

      const metricsBefore = coordinator.getMetrics();
      expect(metricsBefore.completedOperations).toBeGreaterThan(0);

      // Reset
      await coordinator.reset();

      const metricsAfter = coordinator.getMetrics();
      expect(metricsAfter.totalOperations).toBe(0);
      expect(metricsAfter.completedOperations).toBe(0);
      expect(coordinator.isPersisted('node1')).toBe(false);
      expect(coordinator.isPersisted('node2')).toBe(false);
    });
  });

  // ========================================================================
  // Edge Cases
  // ========================================================================

  describe('Edge Cases', () => {
    it('should handle empty dependencies array', async () => {
      let executed = false;

      const handle = coordinator.persist(
        'no-deps-node',
        async () => {
          executed = true;
        },
        {
          mode: 'immediate',
          dependencies: []
        }
      );

      await handle.promise;

      expect(executed).toBe(true);
    });

    it('should handle mixed dependency types', async () => {
      const executionOrder: string[] = [];

      const parent1 = coordinator.persist(
        'parent1',
        async () => {
          executionOrder.push('parent1');
        },
        { mode: 'immediate' }
      );

      const parent2 = coordinator.persist(
        'parent2',
        async () => {
          executionOrder.push('parent2');
        },
        { mode: 'immediate' }
      );

      const child = coordinator.persist(
        'child',
        async () => {
          executionOrder.push('child');
        },
        {
          mode: 'immediate',
          dependencies: [
            'parent1', // String
            parent2, // Handle
            async () => {
              // Lambda
              executionOrder.push('lambda');
            },
            { nodeIds: ['parent1', 'parent2'] } // Batch
          ]
        }
      );

      await Promise.all([parent1.promise, parent2.promise, child.promise]);

      // Child should be last
      expect(executionOrder.indexOf('child')).toBe(executionOrder.length - 1);
      expect(executionOrder).toContain('lambda');
    });

    it('should handle zero debounce time', async () => {
      let executed = false;

      const handle = coordinator.persist(
        'zero-debounce-node',
        async () => {
          executed = true;
        },
        { mode: 'debounce', debounceMs: 0 }
      );

      await handle.promise;

      expect(executed).toBe(true);
    });
  });
});
