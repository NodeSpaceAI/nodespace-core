/**
 * EventBus Performance Test Suite
 *
 * Performance tests for the EventBus system including:
 * - High-frequency event emission
 * - Large subscriber count handling
 * - Memory usage and leak detection
 * - Batching and debouncing efficiency
 * - Real-world usage patterns
 */

import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { EventBus } from '../../lib/services/event-bus';
import type {
  NodeStatusChangedEvent,
  DecorationClickedEvent,
  NodeUpdatedEvent,
  CacheInvalidateEvent
} from '../../lib/services/event-types';
import { waitForEffects } from '../helpers';

describe('EventBus Performance', () => {
  let eventBus: EventBus;

  beforeEach(() => {
    eventBus = new EventBus();
  });

  afterEach(() => {
    eventBus.reset();
  });

  // ========================================================================
  // High-Frequency Event Tests
  // ========================================================================

  describe('High-Frequency Events', () => {
    it('should handle 1000 events efficiently', () => {
      const handler = () => {}; // Minimal handler
      eventBus.subscribe('node:status-changed', handler);

      const startTime = performance.now();

      // Emit 1000 events
      for (let i = 0; i < 1000; i++) {
        eventBus.emit<NodeStatusChangedEvent>({
          type: 'node:status-changed',
          namespace: 'coordination',
          source: 'test',
          nodeId: `node${i % 10}`, // Reuse node IDs to simulate real usage
          status: 'focused'
        });
      }

      const endTime = performance.now();
      const duration = endTime - startTime;

      // Should complete in under 100ms for 1000 events
      expect(duration).toBeLessThan(100);

      const metrics = eventBus.getMetrics();
      expect(metrics.totalEvents).toBe(1000);
      expect(metrics.averageProcessingTime).toBeLessThan(1); // Less than 1ms per event
    });

    it('should maintain performance with event filtering', () => {
      // Set up handlers with different filters
      for (let i = 0; i < 50; i++) {
        eventBus.subscribe('node:status-changed', () => {}, {
          filter: { nodeId: `node${i}` }
        });
      }

      const startTime = performance.now();

      // Emit events that will match different filters
      for (let i = 0; i < 500; i++) {
        eventBus.emit<NodeStatusChangedEvent>({
          type: 'node:status-changed',
          namespace: 'coordination',
          source: 'test',
          nodeId: `node${i % 50}`,
          status: 'focused'
        });
      }

      const endTime = performance.now();
      const duration = endTime - startTime;

      // Should still be efficient even with filtering
      expect(duration).toBeLessThan(200);
    });

    it('should handle rapid subscribe/unsubscribe cycles', () => {
      const startTime = performance.now();

      // Rapidly subscribe and unsubscribe
      for (let i = 0; i < 1000; i++) {
        const unsubscribe = eventBus.subscribe('node:status-changed', () => {});
        unsubscribe();
      }

      const endTime = performance.now();
      const duration = endTime - startTime;

      expect(duration).toBeLessThan(50);

      // Should not leak subscribers
      const counts = eventBus.getSubscriberCounts();
      expect(counts['node:status-changed']).toBeUndefined();
    });
  });

  // ========================================================================
  // Scale Tests
  // ========================================================================

  describe('Scale Tests', () => {
    it('should handle many subscribers efficiently', () => {
      const handlerCount = 1000;
      const callCounts: number[] = Array.from({ length: handlerCount }).fill(0) as number[];

      // Subscribe 1000 handlers
      const startSubscribeTime = performance.now();
      for (let i = 0; i < handlerCount; i++) {
        eventBus.subscribe('node:status-changed', () => {
          callCounts[i]++;
        });
      }
      const subscribeTime = performance.now() - startSubscribeTime;

      expect(subscribeTime).toBeLessThan(50); // Subscription should be fast

      // Emit single event to all handlers
      const startEmitTime = performance.now();
      eventBus.emit<NodeStatusChangedEvent>({
        type: 'node:status-changed',
        namespace: 'coordination',
        source: 'test',
        nodeId: 'node1',
        status: 'focused'
      });
      const emitTime = performance.now() - startEmitTime;

      expect(emitTime).toBeLessThan(100); // Should handle 1000 handlers quickly

      // All handlers should have been called
      expect(callCounts.every((count) => count === 1)).toBe(true);
    });

    it('should maintain performance with large event history', () => {
      // Fill event history to near capacity
      for (let i = 0; i < 950; i++) {
        eventBus.emit<NodeStatusChangedEvent>({
          type: 'node:status-changed',
          namespace: 'coordination',
          source: 'test',
          nodeId: `node${i}`,
          status: 'focused'
        });
      }

      // Performance should not degrade significantly
      const startTime = performance.now();
      for (let i = 0; i < 100; i++) {
        eventBus.emit<NodeStatusChangedEvent>({
          type: 'node:status-changed',
          namespace: 'coordination',
          source: 'test',
          nodeId: `final${i}`,
          status: 'focused'
        });
      }
      const endTime = performance.now();
      const duration = endTime - startTime;

      expect(duration).toBeLessThan(50);

      // History retrieval should still be fast
      const historyStartTime = performance.now();
      const recent = eventBus.getRecentEvents({}, 100);
      const historyTime = performance.now() - historyStartTime;

      expect(historyTime).toBeLessThan(10);
      expect(recent).toHaveLength(100);
    });
  });

  // ========================================================================
  // Batching Performance Tests
  // ========================================================================

  describe('Batching Performance', () => {
    beforeEach(() => {
      eventBus.configureBatching({
        maxBatchSize: 50,
        timeWindowMs: 16,
        enableForTypes: ['node:status-changed']
      });
    });

    it('should improve performance with batching', async () => {
      let handlerCallCount = 0;
      eventBus.subscribe('node:status-changed', () => {
        handlerCallCount++;
      });

      // Emit many events quickly
      const startTime = performance.now();
      for (let i = 0; i < 200; i++) {
        eventBus.emit<NodeStatusChangedEvent>({
          type: 'node:status-changed',
          namespace: 'coordination',
          source: 'test',
          nodeId: `node${i % 20}`,
          status: 'focused'
        });
      }
      const emitTime = performance.now() - startTime;

      // Wait for batch processing
      await waitForEffects(100);

      // Batching should improve emit performance
      expect(emitTime).toBeLessThan(50);

      // All events should eventually be processed
      expect(handlerCallCount).toBe(200);
    });

    it('should handle mixed batched and non-batched events', () => {
      let statusChangeCount = 0;
      let decorationClickCount = 0;

      eventBus.subscribe('node:status-changed', () => {
        statusChangeCount++;
      });
      eventBus.subscribe('decoration:clicked', () => {
        decorationClickCount++;
      });

      const startTime = performance.now();

      // Mix batched and non-batched events
      for (let i = 0; i < 100; i++) {
        // Batched event
        eventBus.emit<NodeStatusChangedEvent>({
          type: 'node:status-changed',
          namespace: 'coordination',
          source: 'test',
          nodeId: `node${i}`,
          status: 'focused'
        });

        // Non-batched event
        eventBus.emit<DecorationClickedEvent>({
          type: 'decoration:clicked',
          namespace: 'interaction',
          source: 'test',
          nodeId: `node${i}`,
          decorationType: 'reference',
          target: 'target',
          clickPosition: { x: 100, y: 200 }
        });
      }

      const endTime = performance.now();
      const duration = endTime - startTime;

      expect(duration).toBeLessThan(100);

      // Non-batched events should be processed immediately
      expect(decorationClickCount).toBe(100);

      // Batched events should also be processed
      expect(statusChangeCount).toBe(100);
    });
  });

  // ========================================================================
  // Debouncing Performance Tests
  // ========================================================================

  describe('Debouncing Performance', () => {
    it('should reduce handler calls with debouncing', async () => {
      let handlerCallCount = 0;
      eventBus.subscribe(
        'node:status-changed',
        () => {
          handlerCallCount++;
        },
        { debounceMs: 50 }
      );

      // Rapidly emit events
      const startTime = performance.now();
      for (let i = 0; i < 100; i++) {
        eventBus.emit<NodeStatusChangedEvent>({
          type: 'node:status-changed',
          namespace: 'coordination',
          source: 'test',
          nodeId: 'node1',
          status: 'focused'
        });
      }
      const emitTime = performance.now() - startTime;

      // Emissions should be fast
      expect(emitTime).toBeLessThan(50);

      // Initially no handler calls
      expect(handlerCallCount).toBe(0);

      // Wait for debounce
      await waitForEffects(100);

      // Should only be called once due to debouncing
      expect(handlerCallCount).toBe(1);
    });
  });

  // ========================================================================
  // Memory Performance Tests
  // ========================================================================

  describe('Memory Performance', () => {
    it('should not leak memory with many subscriptions', () => {
      const initialMetrics = eventBus.getMetrics();
      const unsubscribers: (() => void)[] = [];

      // Create many subscriptions
      for (let i = 0; i < 1000; i++) {
        const unsubscribe = eventBus.subscribe('node:status-changed', () => {});
        unsubscribers.push(unsubscribe);
      }

      expect(eventBus.getMetrics().totalHandlers).toBe(1000);

      // Unsubscribe all
      unsubscribers.forEach((unsub) => unsub());

      // Should return to baseline
      expect(eventBus.getMetrics().totalHandlers).toBe(initialMetrics.totalHandlers);
    });

    it('should limit event history size', () => {
      // Emit more events than history limit
      for (let i = 0; i < 2000; i++) {
        eventBus.emit<NodeStatusChangedEvent>({
          type: 'node:status-changed',
          namespace: 'coordination',
          source: 'test',
          nodeId: `node${i}`,
          status: 'focused'
        });
      }

      // History should be limited (EventBus default is 1000)
      const history = eventBus.getRecentEvents({}, 2000);
      expect(history.length).toBeLessThanOrEqual(1000);

      // Most recent events should be preserved
      const lastEvent = history[0] as NodeStatusChangedEvent;
      expect(lastEvent.nodeId).toBe('node1999');
    });

    it('should clean up timeouts and resources', () => {
      // Set up debounced subscriptions
      for (let i = 0; i < 100; i++) {
        eventBus.subscribe('node:status-changed', () => {}, { debounceMs: 100 });
      }

      // Emit events to create timeouts
      for (let i = 0; i < 100; i++) {
        eventBus.emit<NodeStatusChangedEvent>({
          type: 'node:status-changed',
          namespace: 'coordination',
          source: 'test',
          nodeId: `node${i}`,
          status: 'focused'
        });
      }

      // Reset should clean up everything
      eventBus.reset();

      // Should not have any active subscriptions or timeouts
      const metrics = eventBus.getMetrics();
      expect(metrics.totalHandlers).toBe(0);
      expect(metrics.totalEvents).toBe(0);
    });
  });

  // ========================================================================
  // Real-World Usage Patterns
  // ========================================================================

  describe('Real-World Usage Patterns', () => {
    it('should handle mixed event types efficiently', () => {
      const handlers = {
        statusChanged: () => {},
        nodeCreated: () => {},
        nodeUpdated: () => {},
        decorationClicked: () => {},
        cacheInvalidate: () => {}
      };

      // Set up multiple event type handlers
      eventBus.subscribe('node:status-changed', handlers.statusChanged);
      eventBus.subscribe('node:created', handlers.nodeCreated);
      eventBus.subscribe('node:updated', handlers.nodeUpdated);
      eventBus.subscribe('decoration:clicked', handlers.decorationClicked);
      eventBus.subscribe('cache:invalidate', handlers.cacheInvalidate);

      const startTime = performance.now();

      // Simulate typical usage pattern
      for (let i = 0; i < 100; i++) {
        // Status changes (frequent)
        eventBus.emit<NodeStatusChangedEvent>({
          type: 'node:status-changed',
          namespace: 'coordination',
          source: 'test',
          nodeId: `node${i % 10}`,
          status: 'focused'
        });

        // Occasional content updates
        if (i % 5 === 0) {
          eventBus.emit<NodeUpdatedEvent>({
            type: 'node:updated',
            namespace: 'lifecycle',
            source: 'test',
            nodeId: `node${i}`,
            updateType: 'content',
            newValue: 'updated'
          });
        }

        // Occasional interactions
        if (i % 10 === 0) {
          eventBus.emit<DecorationClickedEvent>({
            type: 'decoration:clicked',
            namespace: 'interaction',
            source: 'test',
            nodeId: `node${i}`,
            decorationType: 'reference',
            target: 'target',
            clickPosition: { x: 100, y: 200 }
          });
        }

        // Cache invalidations
        if (i % 7 === 0) {
          eventBus.emit<CacheInvalidateEvent>({
            type: 'cache:invalidate',
            namespace: 'coordination',
            source: 'test',
            cacheKey: `cache${i}`,
            scope: 'node',
            reason: 'test'
          });
        }
      }

      const endTime = performance.now();
      const duration = endTime - startTime;

      // Should handle mixed workload efficiently
      expect(duration).toBeLessThan(100);

      const metrics = eventBus.getMetrics();
      expect(metrics.totalEvents).toBeGreaterThan(100);
    });

    it('should maintain performance under sustained load', async () => {
      let totalHandlerCalls = 0;

      // Set up handlers
      eventBus.subscribe('*', () => {
        totalHandlerCalls++;
      });

      const iterations = 5;
      const eventsPerIteration = 100;
      const durations: number[] = [];

      // Sustained load test
      for (let iteration = 0; iteration < iterations; iteration++) {
        const startTime = performance.now();

        for (let i = 0; i < eventsPerIteration; i++) {
          eventBus.emit<NodeStatusChangedEvent>({
            type: 'node:status-changed',
            namespace: 'coordination',
            source: 'test',
            nodeId: `node${i}`,
            status: 'focused'
          });
        }

        const endTime = performance.now();
        durations.push(endTime - startTime);

        // Small delay between iterations
        await waitForEffects(10);
      }

      // Performance should remain consistent across iterations
      const avgDuration = durations.reduce((sum, d) => sum + d, 0) / durations.length;
      const maxDuration = Math.max(...durations);

      expect(avgDuration).toBeLessThan(50);
      expect(maxDuration - avgDuration).toBeLessThan(20); // Low variance

      expect(totalHandlerCalls).toBe(iterations * eventsPerIteration);
    });
  });
});
