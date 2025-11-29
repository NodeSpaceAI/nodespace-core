/**
 * Subscription Cleanup Test (Issue #640)
 *
 * Tests that ReactiveNodeService properly cleans up subscriptions when destroyed.
 * This prevents memory leaks from accumulating subscriptions when viewers mount/unmount.
 *
 * Background:
 * - Each ReactiveNodeService instance creates a wildcard subscription to SharedNodeStore
 * - Without proper cleanup, subscriptions accumulate (1 per mount)
 * - This test verifies the destroy() method properly unsubscribes
 */

// Mock Svelte 5 runes immediately before any imports - using proper type assertions
(globalThis as Record<string, unknown>).$state = function <T>(initialValue: T): T {
  if (typeof initialValue !== 'object' || initialValue === null) {
    return initialValue;
  }
  return initialValue;
};

(globalThis as Record<string, unknown>).$derived = {
  by: function <T>(getter: () => T): T {
    return getter();
  }
};

(globalThis as Record<string, unknown>).$effect = function (fn: () => void | (() => void)): void {
  fn();
};

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  createReactiveNodeService,
  type NodeManagerEvents
} from '$lib/services/reactive-node-service.svelte';
import { SharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { createMockNodeManagerEvents, createTestNode } from '../helpers/test-helpers';

describe('ReactiveNodeService Subscription Cleanup (Issue #640)', () => {
  let mockEvents: NodeManagerEvents;

  beforeEach(() => {
    mockEvents = createMockNodeManagerEvents();
    // Reset SharedNodeStore to ensure clean metrics
    SharedNodeStore.resetInstance();
  });

  afterEach(() => {
    // Clean up after each test
    SharedNodeStore.resetInstance();
  });

  describe('destroy() method', () => {
    it('should clean up subscription when destroy() is called', () => {
      const store = SharedNodeStore.getInstance();
      const initialSubscriptionCount = store.getMetrics().subscriptionCount;

      // Create a service instance (creates a subscription)
      const service = createReactiveNodeService(mockEvents);

      // Verify subscription was created
      const afterCreateCount = store.getMetrics().subscriptionCount;
      expect(afterCreateCount).toBe(initialSubscriptionCount + 1);

      // Destroy the service
      service.destroy();

      // Verify subscription was cleaned up
      const afterDestroyCount = store.getMetrics().subscriptionCount;
      expect(afterDestroyCount).toBe(initialSubscriptionCount);
    });

    it('should be safe to call destroy() multiple times', () => {
      const store = SharedNodeStore.getInstance();
      const initialSubscriptionCount = store.getMetrics().subscriptionCount;

      const service = createReactiveNodeService(mockEvents);
      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount + 1);

      // Call destroy multiple times
      service.destroy();
      service.destroy();
      service.destroy();

      // Should still only decrement once
      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount);
    });

    it('should prevent memory leak after repeated mount/unmount cycles', () => {
      const store = SharedNodeStore.getInstance();
      const initialSubscriptionCount = store.getMetrics().subscriptionCount;

      // Simulate 100 mount/unmount cycles (the issue scenario)
      for (let i = 0; i < 100; i++) {
        const service = createReactiveNodeService(mockEvents);
        service.destroy();
      }

      // Subscription count should be back to initial
      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount);
    });

    it('should stop receiving callbacks after destroy()', () => {
      const store = SharedNodeStore.getInstance();
      const service = createReactiveNodeService(mockEvents);

      // Initialize with a node
      const testNode = createTestNode({ id: 'test-node', content: 'Test' });
      service.initializeNodes([testNode]);

      // Get initial update trigger
      const initialTrigger = service._updateTrigger;

      // Destroy the service
      service.destroy();

      // Update a node in the store (should not trigger callback)
      store.updateNode('test-node', { content: 'Updated' }, { type: 'database', reason: 'test' });

      // Update trigger should NOT have incremented (callback not called)
      // Note: The exact behavior depends on the subscription callback,
      // but at minimum, no errors should occur
      expect(service._updateTrigger).toBe(initialTrigger);
    });
  });

  describe('subscription accumulation prevention', () => {
    it('should not accumulate subscriptions without destroy() (demonstrating the bug)', () => {
      const store = SharedNodeStore.getInstance();
      const initialSubscriptionCount = store.getMetrics().subscriptionCount;

      // Create multiple services WITHOUT destroying them
      const services: ReturnType<typeof createReactiveNodeService>[] = [];
      for (let i = 0; i < 10; i++) {
        services.push(createReactiveNodeService(mockEvents));
      }

      // Subscriptions should accumulate (the bug behavior)
      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount + 10);

      // Clean up for the test
      for (const service of services) {
        service.destroy();
      }

      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount);
    });

    it('should properly clean up when services are destroyed in order', () => {
      const store = SharedNodeStore.getInstance();
      const initialSubscriptionCount = store.getMetrics().subscriptionCount;

      const service1 = createReactiveNodeService(mockEvents);
      const service2 = createReactiveNodeService(mockEvents);
      const service3 = createReactiveNodeService(mockEvents);

      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount + 3);

      // Destroy in order
      service1.destroy();
      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount + 2);

      service2.destroy();
      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount + 1);

      service3.destroy();
      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount);
    });

    it('should properly clean up when services are destroyed in reverse order', () => {
      const store = SharedNodeStore.getInstance();
      const initialSubscriptionCount = store.getMetrics().subscriptionCount;

      const service1 = createReactiveNodeService(mockEvents);
      const service2 = createReactiveNodeService(mockEvents);
      const service3 = createReactiveNodeService(mockEvents);

      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount + 3);

      // Destroy in reverse order
      service3.destroy();
      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount + 2);

      service2.destroy();
      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount + 1);

      service1.destroy();
      expect(store.getMetrics().subscriptionCount).toBe(initialSubscriptionCount);
    });
  });
});
