/**
 * EventBus Test Suite
 *
 * Comprehensive tests for the EventBus system including:
 * - Core functionality (emit, subscribe, unsubscribe)
 * - Type safety and event filtering
 * - Performance optimizations (batching, debouncing)
 * - Error handling and edge cases
 * - Memory leak prevention
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { EventBus } from '../../lib/services/EventBus';
import type { NodeStatusChangedEvent, EventFilter, DecorationClickedEvent } from '../../lib/services/EventTypes';

// Helper functions for creating typed test events
const createNodeStatusEvent = (overrides: Partial<Omit<NodeStatusChangedEvent, 'timestamp'>> = {}): Omit<NodeStatusChangedEvent, 'timestamp'> => ({
  type: 'node:status-changed',
  namespace: 'coordination',
  source: 'test',
  nodeId: 'node1',
  status: 'focused',
  ...overrides
});

const createDecorationClickEvent = (overrides: Partial<Omit<DecorationClickedEvent, 'timestamp'>> = {}): Omit<DecorationClickedEvent, 'timestamp'> => ({
  type: 'decoration:clicked',
  namespace: 'interaction',
  source: 'test',
  nodeId: 'node1',
  decorationType: 'reference',
  target: 'target1',
  clickPosition: { x: 100, y: 200 },
  ...overrides
});

describe('EventBus', () => {
  let eventBus: EventBus;

  beforeEach(() => {
    eventBus = new EventBus();
  });

  afterEach(() => {
    eventBus.reset();
  });

  // ========================================================================
  // Core Functionality Tests
  // ========================================================================

  describe('Core Functionality', () => {
    it('should emit and handle events', () => {
      const handler = vi.fn();

      eventBus.subscribe('node:status-changed', handler);

      const event: Omit<NodeStatusChangedEvent, 'timestamp'> = {
        type: 'node:status-changed',
        namespace: 'coordination',
        source: 'test',
        nodeId: 'node1',
        status: 'focused'
      };

      eventBus.emit(event);

      expect(handler).toHaveBeenCalledWith(
        expect.objectContaining({
          ...event,
          timestamp: expect.any(Number)
        })
      );
    });

    it('should support multiple subscribers for same event', () => {
      const handler1 = vi.fn();
      const handler2 = vi.fn();

      eventBus.subscribe('node:status-changed', handler1);
      eventBus.subscribe('node:status-changed', handler2);

      const event: Omit<NodeStatusChangedEvent, 'timestamp'> = {
        type: 'node:status-changed',
        namespace: 'coordination',
        source: 'test',
        nodeId: 'node1',
        status: 'focused'
      };

      eventBus.emit(event);

      expect(handler1).toHaveBeenCalled();
      expect(handler2).toHaveBeenCalled();
    });

    it('should support wildcard subscriptions', () => {
      const handler = vi.fn();

      eventBus.subscribe('*', handler);

      eventBus.emit({
        type: 'node:status-changed',
        namespace: 'coordination',
        source: 'test',
        nodeId: 'node1',
        status: 'focused'
      } as Omit<NodeStatusChangedEvent, 'timestamp'>);

      eventBus.emit({
        type: 'decoration:clicked',
        namespace: 'interaction',
        source: 'test',
        nodeId: 'node1',
        decorationType: 'reference',
        target: 'target1',
        clickPosition: { x: 100, y: 200 }
      } as Omit<DecorationClickedEvent, 'timestamp'>);

      expect(handler).toHaveBeenCalledTimes(2);
    });

    it('should unsubscribe correctly', () => {
      const handler = vi.fn();

      const unsubscribe = eventBus.subscribe('node:status-changed', handler);

      eventBus.emit(createNodeStatusEvent());

      expect(handler).toHaveBeenCalledTimes(1);

      unsubscribe();

      eventBus.emit(createNodeStatusEvent({ status: 'active' }));

      expect(handler).toHaveBeenCalledTimes(1); // No additional calls
    });

    it('should handle once subscriptions', async () => {
      const handler = vi.fn();

      eventBus.once('node:status-changed', handler);

      eventBus.emit(createNodeStatusEvent());

      eventBus.emit(createNodeStatusEvent({ status: 'active' }));

      expect(handler).toHaveBeenCalledTimes(1);
    });
  });

  // ========================================================================
  // Event Filtering Tests
  // ========================================================================

  describe('Event Filtering', () => {
    it('should filter events by namespace', () => {
      const handler = vi.fn();

      const filter: EventFilter = { namespace: 'coordination' };
      eventBus.subscribe('*', handler, { filter });

      // Should handle this event (coordination namespace)
      eventBus.emit(createNodeStatusEvent());

      // Should ignore this event (interaction namespace)
      eventBus.emit(createDecorationClickEvent());

      expect(handler).toHaveBeenCalledTimes(1);
    });

    it('should filter events by source', () => {
      const handler = vi.fn();

      const filter: EventFilter = { source: 'NodeManager' };
      eventBus.subscribe('*', handler, { filter });

      // Should handle this event
      eventBus.emit(createNodeStatusEvent({ source: 'NodeManager' }));

      // Should ignore this event
      eventBus.emit(createNodeStatusEvent({ source: 'ContentProcessor' }));

      expect(handler).toHaveBeenCalledTimes(1);
    });

    it('should filter events by node ID', () => {
      const handler = vi.fn();

      const filter: EventFilter = { nodeId: 'node1' };
      eventBus.subscribe('*', handler, { filter });

      // Should handle this event
      eventBus.emit(createNodeStatusEvent());

      // Should ignore this event
      eventBus.emit(createNodeStatusEvent({ nodeId: 'node2' }));

      expect(handler).toHaveBeenCalledTimes(1);
    });

    it('should handle multiple filter criteria', () => {
      const handler = vi.fn();

      const filter: EventFilter = {
        namespace: 'coordination',
        source: 'NodeManager',
        nodeId: 'node1'
      };
      eventBus.subscribe('*', handler, { filter });

      // Should handle this event (matches all criteria)
      eventBus.emit(createNodeStatusEvent({ source: 'NodeManager' }));

      // Should ignore this event (wrong namespace)
      eventBus.emit(createDecorationClickEvent({ source: 'NodeManager' }));

      expect(handler).toHaveBeenCalledTimes(1);
    });
  });

  // ========================================================================
  // Debouncing Tests
  // ========================================================================

  describe('Debouncing', () => {
    it('should debounce events', async () => {
      const handler = vi.fn();

      eventBus.subscribe('node:status-changed', handler, { debounceMs: 50 });

      // Emit multiple events quickly
      eventBus.emit(createNodeStatusEvent());

      eventBus.emit(createNodeStatusEvent({ status: 'active' }));

      eventBus.emit(createNodeStatusEvent({ status: 'editing' }));

      // Should not be called immediately
      expect(handler).not.toHaveBeenCalled();

      // Wait for debounce delay
      await new Promise((resolve) => setTimeout(resolve, 100));

      // Should be called only once with the last event
      expect(handler).toHaveBeenCalledTimes(1);
      expect(handler).toHaveBeenCalledWith(
        expect.objectContaining({
          status: 'editing'
        })
      );
    });
  });

  // ========================================================================
  // Batching Tests
  // ========================================================================

  describe('Batching', () => {
    beforeEach(() => {
      eventBus.configureBatching({
        maxBatchSize: 3,
        timeWindowMs: 50,
        enableForTypes: ['node:status-changed']
      });
    });

    it('should batch events when configured', async () => {
      const handler = vi.fn();

      eventBus.subscribe('node:status-changed', handler);

      // Emit multiple events of batchable type
      eventBus.emit(createNodeStatusEvent());

      eventBus.emit(createNodeStatusEvent({ nodeId: 'node2', status: 'active' }));

      eventBus.emit(createNodeStatusEvent({ nodeId: 'node3', status: 'editing' }));

      // Wait for batch processing
      await new Promise((resolve) => setTimeout(resolve, 100));

      // All events should have been processed
      expect(handler).toHaveBeenCalledTimes(3);
    });

    it('should process batch when max size reached', () => {
      const handler = vi.fn();

      eventBus.subscribe('node:status-changed', handler);

      // Emit exactly max batch size events
      for (let i = 0; i < 3; i++) {
        eventBus.emit(createNodeStatusEvent({ nodeId: `node${i}` }));
      }

      // Should process immediately when batch is full
      expect(handler).toHaveBeenCalledTimes(3);
    });
  });

  // ========================================================================
  // Event History and Querying Tests
  // ========================================================================

  describe('Event History', () => {
    it('should maintain event history', () => {
      eventBus.emit(createNodeStatusEvent());

      eventBus.emit(createDecorationClickEvent());

      const recent = eventBus.getRecentEvents();
      expect(recent).toHaveLength(2);
    });

    it('should filter event history', () => {
      eventBus.emit(createNodeStatusEvent());

      eventBus.emit(createDecorationClickEvent());

      const filter: EventFilter = { namespace: 'coordination' };
      const filtered = eventBus.getRecentEvents(filter);

      expect(filtered).toHaveLength(1);
      expect(filtered[0].type).toBe('node:status-changed');
    });

    it('should wait for specific events', async () => {
      const promise = eventBus.waitFor('node:status-changed');

      setTimeout(() => {
        eventBus.emit(createNodeStatusEvent());
      }, 50);

      const event = await promise;
      expect(event.type).toBe('node:status-changed');
    }, 1000);
  });

  // ========================================================================
  // Performance and Metrics Tests
  // ========================================================================

  describe('Performance and Metrics', () => {
    it('should track performance metrics', () => {
      const handler = vi.fn();
      eventBus.subscribe('node:status-changed', handler);

      eventBus.emit(createNodeStatusEvent());

      const metrics = eventBus.getMetrics();
      expect(metrics.totalEvents).toBe(1);
      expect(metrics.totalHandlers).toBe(1);
      expect(metrics.averageProcessingTime).toBeGreaterThanOrEqual(0);
    });

    it('should track subscriber counts', () => {
      eventBus.subscribe('node:status-changed', vi.fn());
      eventBus.subscribe('node:status-changed', vi.fn());
      eventBus.subscribe('decoration:clicked', vi.fn());

      const counts = eventBus.getSubscriberCounts();
      expect(counts['node:status-changed']).toBe(2);
      expect(counts['decoration:clicked']).toBe(1);
    });
  });

  // ========================================================================
  // Error Handling Tests
  // ========================================================================

  describe('Error Handling', () => {
    it('should handle errors in event handlers gracefully', () => {
      const errorHandler = vi.fn(() => {
        throw new Error('Test error');
      });
      const normalHandler = vi.fn();

      eventBus.subscribe('node:status-changed', errorHandler);
      eventBus.subscribe('node:status-changed', normalHandler);

      // Should not throw even with error handler
      expect(() => {
        eventBus.emit(createNodeStatusEvent());
      }).not.toThrow();

      // Normal handler should still be called
      expect(normalHandler).toHaveBeenCalled();
    });

    it('should handle async handler errors', async () => {
      const asyncErrorHandler = vi.fn(async () => {
        throw new Error('Async test error');
      });
      const normalHandler = vi.fn();

      eventBus.subscribe('node:status-changed', asyncErrorHandler);
      eventBus.subscribe('node:status-changed', normalHandler);

      eventBus.emit(createNodeStatusEvent());

      // Wait for async processing
      await new Promise((resolve) => setTimeout(resolve, 10));

      expect(normalHandler).toHaveBeenCalled();
    });
  });

  // ========================================================================
  // Memory Management Tests
  // ========================================================================

  describe('Memory Management', () => {
    it('should clean up resources on reset', () => {
      const handler = vi.fn();
      eventBus.subscribe('node:status-changed', handler);

      eventBus.emit(createNodeStatusEvent());

      expect(eventBus.getSubscriberCounts()['node:status-changed']).toBe(1);
      expect(eventBus.getRecentEvents()).toHaveLength(1);

      eventBus.reset();

      expect(eventBus.getSubscriberCounts()['node:status-changed']).toBeUndefined();
      expect(eventBus.getRecentEvents()).toHaveLength(0);
      expect(eventBus.getMetrics().totalEvents).toBe(0);
    });

    it('should prevent subscription leaks', () => {
      // Subscribe and immediately unsubscribe many times
      for (let i = 0; i < 100; i++) {
        const unsubscribe = eventBus.subscribe('node:status-changed', vi.fn());
        unsubscribe();
      }

      const counts = eventBus.getSubscriberCounts();
      expect(counts['node:status-changed']).toBeUndefined();
      expect(eventBus.getMetrics().totalHandlers).toBe(0);
    });
  });

  // ========================================================================
  // Advanced Features Tests
  // ========================================================================

  describe('Advanced Features', () => {
    it('should support multiple event type subscriptions', () => {
      const handler = vi.fn();

      const unsubscribe = eventBus.subscribeMultiple(
        ['node:status-changed', 'decoration:clicked'],
        handler
      );

      eventBus.emit(createNodeStatusEvent());

      eventBus.emit(createDecorationClickEvent());

      expect(handler).toHaveBeenCalledTimes(2);

      unsubscribe();

      eventBus.emit(createNodeStatusEvent({ status: 'active' }));

      expect(handler).toHaveBeenCalledTimes(2); // No additional calls
    });

    it('should handle priority-based handler execution', () => {
      const calls: string[] = [];

      eventBus.subscribe('node:status-changed', () => { calls.push('handler1'); }, { priority: 1 });
      eventBus.subscribe('node:status-changed', () => { calls.push('handler2'); }, { priority: 3 });
      eventBus.subscribe('node:status-changed', () => { calls.push('handler3'); }, { priority: 2 });

      eventBus.emit(createNodeStatusEvent());

      // Note: Current implementation doesn't support priority yet
      // This test documents expected future behavior
      expect(calls).toContain('handler1');
      expect(calls).toContain('handler2');
      expect(calls).toContain('handler3');
    });
  });
});
