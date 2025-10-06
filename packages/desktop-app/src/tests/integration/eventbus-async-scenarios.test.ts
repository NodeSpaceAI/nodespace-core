/**
 * EventBus Async Scenarios and Race Condition Test Suite
 *
 * Tests async handler behavior, event ordering, and race conditions:
 * - Rapid event sequences maintain order
 * - Async handlers coordinate properly
 * - Concurrent operations don't conflict
 * - Timing edge cases handled gracefully
 *
 * Addresses Issue #160: Comprehensive async event testing
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { eventBus } from '../../lib/services/eventBus';
import { waitForEffects } from '../helpers';
import type { NodeUpdatedEvent, NodeCreatedEvent } from '../../lib/services/eventTypes';

// Test timing constants for maintainability and consistency
const ASYNC_HANDLER_DELAY = 100;
const ASYNC_ERROR_SETTLE = 10;
const BATCH_PROCESSING_DELAY = 10;
const BATCH_COMPLETION_WAIT = 50;
const DEBOUNCE_DELAY = 100;
const BATCH_DELAY = 50;

describe('EventBus Async and Race Conditions', () => {
  beforeEach(() => {
    eventBus.reset();
    vi.clearAllMocks();
  });

  afterEach(() => {
    eventBus.reset();
    // Clean up timers if any tests failed before cleanup
    if (vi.isFakeTimers()) {
      vi.useRealTimers();
    }
  });

  // ========================================================================
  // Rapid Event Sequences
  // ========================================================================

  describe('Rapid Event Sequences', () => {
    it('should not drop events during fast typing', async () => {
      const processedEvents: string[] = [];

      eventBus.subscribe('node:updated', (event: NodeUpdatedEvent) => {
        processedEvents.push(event.nodeId);
      });

      // Simulate rapid typing - 10 events fired immediately
      for (let i = 0; i < 10; i++) {
        eventBus.emit<NodeUpdatedEvent>({
          type: 'node:updated',
          namespace: 'lifecycle',
          source: 'test',
          nodeId: `node-${i}`,
          updateType: 'content',
          newValue: `content-${i}`
        });
      }

      await waitForEffects();

      // All events should be processed, none dropped
      expect(processedEvents).toHaveLength(10);
    });

    it('should maintain event order during rapid updates', async () => {
      const processedOrder: string[] = [];

      eventBus.subscribe('node:updated', (event: NodeUpdatedEvent) => {
        processedOrder.push(event.nodeId);
      });

      // Rapidly fire events in specific order
      for (let i = 0; i < 10; i++) {
        eventBus.emit<NodeUpdatedEvent>({
          type: 'node:updated',
          namespace: 'lifecycle',
          source: 'test',
          nodeId: `node-${i}`,
          updateType: 'content',
          newValue: `content-${i}`
        });
      }

      await waitForEffects();

      // Order should be preserved exactly
      expect(processedOrder).toEqual([
        'node-0',
        'node-1',
        'node-2',
        'node-3',
        'node-4',
        'node-5',
        'node-6',
        'node-7',
        'node-8',
        'node-9'
      ]);
    });

    it('should preserve latest event with debouncing pattern', async () => {
      vi.useFakeTimers();
      let latestValue: string | null = null;
      let debounceTimer: ReturnType<typeof setTimeout> | null = null;

      // Debounced handler pattern
      eventBus.subscribe('node:updated', (event: NodeUpdatedEvent) => {
        if (debounceTimer) {
          clearTimeout(debounceTimer);
        }
        debounceTimer = setTimeout(() => {
          latestValue = event.newValue as string;
        }, DEBOUNCE_DELAY);
      });

      // Rapid fire 10 events
      for (let i = 0; i < 10; i++) {
        eventBus.emit<NodeUpdatedEvent>({
          type: 'node:updated',
          namespace: 'lifecycle',
          source: 'test',
          nodeId: 'node-1',
          updateType: 'content',
          newValue: `content-${i}`
        });
      }

      // Fast-forward past debounce delay
      await vi.runAllTimersAsync();

      // Only the last event should be processed
      expect(latestValue).toBe('content-9');

      vi.useRealTimers();
    });

    it('should process all events in batch', async () => {
      const batchedEvents: NodeUpdatedEvent[] = [];
      let batchTimer: ReturnType<typeof setTimeout> | null = null;

      vi.useFakeTimers();

      // Batching handler pattern
      eventBus.subscribe('node:updated', (event: NodeUpdatedEvent) => {
        batchedEvents.push(event);

        if (batchTimer) {
          clearTimeout(batchTimer);
        }

        batchTimer = setTimeout(() => {
          // Process batch
          expect(batchedEvents.length).toBeGreaterThan(0);
        }, BATCH_DELAY);
      });

      // Fire multiple events rapidly
      for (let i = 0; i < 5; i++) {
        eventBus.emit<NodeUpdatedEvent>({
          type: 'node:updated',
          namespace: 'lifecycle',
          source: 'test',
          nodeId: `node-${i}`,
          updateType: 'content',
          newValue: `content-${i}`
        });
      }

      await vi.runAllTimersAsync();

      // All events should be in the batch
      expect(batchedEvents).toHaveLength(5);
      expect(batchedEvents.map((e) => e.nodeId)).toEqual([
        'node-0',
        'node-1',
        'node-2',
        'node-3',
        'node-4'
      ]);

      vi.useRealTimers();
    });
  });

  // ========================================================================
  // Async Handler Coordination
  // ========================================================================

  describe('Async Handler Coordination', () => {
    it('should not block other handlers when one is slow', async () => {
      vi.useFakeTimers();
      const executionOrder: string[] = [];

      // Slow async handler
      eventBus.subscribe('node:updated', async () => {
        executionOrder.push('slow-start');
        await new Promise((resolve) => setTimeout(resolve, ASYNC_HANDLER_DELAY));
        executionOrder.push('slow-end');
      });

      // Fast sync handler
      eventBus.subscribe('node:updated', () => {
        executionOrder.push('fast');
      });

      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'test',
        nodeId: 'test',
        updateType: 'content',
        newValue: 'value'
      });

      await waitForEffects();

      // Both handlers execute their synchronous portions immediately in registration order
      expect(executionOrder[0]).toBe('slow-start');
      expect(executionOrder[1]).toBe('fast');

      // Fast-forward timers to complete slow handler
      await vi.runAllTimersAsync();
      expect(executionOrder[2]).toBe('slow-end');

      vi.useRealTimers();
    });

    it('should handle async errors gracefully without breaking event flow', async () => {
      const asyncErrorHandler = vi.fn(async () => {
        throw new Error('Async handler failed');
      });
      const successHandler = vi.fn();

      eventBus.subscribe('node:updated', asyncErrorHandler);
      eventBus.subscribe('node:updated', successHandler);

      // Suppress only EventBus async handler errors
      const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation((msg, ...args) => {
        if (typeof msg === 'string' && !msg.includes('EventBus: Async handler error')) {
          // Log unexpected errors for debugging
          console.warn('Unexpected console.error:', msg, ...args);
        }
      });

      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'test',
        nodeId: 'test',
        updateType: 'content',
        newValue: 'value'
      });

      await waitForEffects();
      await new Promise((resolve) => setTimeout(resolve, ASYNC_ERROR_SETTLE));

      // Success handler should still be called despite error
      expect(successHandler).toHaveBeenCalled();

      // Error should have been logged
      expect(consoleErrorSpy).toHaveBeenCalledWith(
        'EventBus: Async handler error',
        expect.objectContaining({
          eventType: 'node:updated'
        })
      );

      consoleErrorSpy.mockRestore();
    });

    it('should handle promise rejections properly', async () => {
      const rejectedHandler = vi.fn(async () => {
        return Promise.reject(new Error('Promise rejected'));
      });
      const otherHandler = vi.fn();

      eventBus.subscribe('node:created', rejectedHandler);
      eventBus.subscribe('node:created', otherHandler);

      // Suppress only EventBus async handler errors
      const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation((msg, ...args) => {
        if (typeof msg === 'string' && !msg.includes('EventBus: Async handler error')) {
          // Log unexpected errors for debugging
          console.warn('Unexpected console.error:', msg, ...args);
        }
      });

      eventBus.emit<NodeCreatedEvent>({
        type: 'node:created',
        namespace: 'lifecycle',
        source: 'test',
        nodeId: 'test',
        nodeType: 'text'
      });

      await waitForEffects();
      await new Promise((resolve) => setTimeout(resolve, ASYNC_ERROR_SETTLE));

      // Both handlers should have been called
      expect(rejectedHandler).toHaveBeenCalled();
      expect(otherHandler).toHaveBeenCalled();

      // Rejection should have been logged
      expect(consoleErrorSpy).toHaveBeenCalled();

      consoleErrorSpy.mockRestore();
    });
  });

  // ========================================================================
  // Concurrent Operations
  // ========================================================================

  describe('Concurrent Operations', () => {
    it('should handle multiple simultaneous updates without conflicts', async () => {
      const updates = new Map<string, string>();

      eventBus.subscribe('node:updated', (event: NodeUpdatedEvent) => {
        updates.set(event.nodeId, event.newValue as string);
      });

      // Fire multiple updates simultaneously
      const nodeIds = ['node-1', 'node-2', 'node-3', 'node-4', 'node-5'];
      nodeIds.forEach((nodeId) => {
        eventBus.emit<NodeUpdatedEvent>({
          type: 'node:updated',
          namespace: 'lifecycle',
          source: 'test',
          nodeId,
          updateType: 'content',
          newValue: `content-${nodeId}`
        });
      });

      await waitForEffects();

      // All updates should be recorded without conflicts
      expect(updates.size).toBe(5);
      nodeIds.forEach((nodeId) => {
        expect(updates.get(nodeId)).toBe(`content-${nodeId}`);
      });
    });

    it('should maintain event ordering under concurrent load', async () => {
      const eventLog: Array<{ nodeId: string; sequence: number }> = [];

      eventBus.subscribe('node:updated', (event: NodeUpdatedEvent) => {
        const sequence = parseInt((event.newValue as string).split('-')[1]);
        eventLog.push({ nodeId: event.nodeId, sequence });
      });

      // Create interleaved events from multiple nodes
      for (let i = 0; i < 20; i++) {
        const nodeId = `node-${i % 4}`; // Rotate through 4 nodes
        eventBus.emit<NodeUpdatedEvent>({
          type: 'node:updated',
          namespace: 'lifecycle',
          source: 'test',
          nodeId,
          updateType: 'content',
          newValue: `update-${i}`
        });
      }

      await waitForEffects();

      // Events should be in submission order
      expect(eventLog).toHaveLength(20);
      for (let i = 0; i < 20; i++) {
        expect(eventLog[i].sequence).toBe(i);
      }
    });

    it('should maintain acceptable performance under load', async () => {
      const handler = vi.fn();
      eventBus.subscribe('node:updated', handler);

      const startTime = performance.now();

      // Fire 100 rapid updates to stress test the system
      for (let i = 0; i < 100; i++) {
        eventBus.emit<NodeUpdatedEvent>({
          type: 'node:updated',
          namespace: 'lifecycle',
          source: 'test',
          nodeId: `node-${i}`,
          updateType: 'content',
          newValue: `content-${i}`
        });
      }

      await waitForEffects();

      const duration = performance.now() - startTime;

      // Should complete in under 100ms (per acceptance criteria)
      // 100 synchronous events with synchronous handlers should be very fast
      expect(duration).toBeLessThan(100);
      expect(handler).toHaveBeenCalledTimes(100);
    });
  });

  // ========================================================================
  // Timing Edge Cases
  // ========================================================================

  describe('Timing Edge Cases', () => {
    it('should handle events during unmount gracefully', async () => {
      const handler = vi.fn();
      const unsubscribe = eventBus.subscribe('node:updated', handler);

      // Emit event
      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'test',
        nodeId: 'test',
        updateType: 'content',
        newValue: 'value-1'
      });

      await waitForEffects();
      expect(handler).toHaveBeenCalledTimes(1);

      // Unsubscribe (simulate unmount)
      unsubscribe();

      // Emit another event after unmount
      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'test',
        nodeId: 'test',
        updateType: 'content',
        newValue: 'value-2'
      });

      await waitForEffects();

      // Handler should not be called again
      expect(handler).toHaveBeenCalledTimes(1);
    });

    it('should handle events during async handler processing', async () => {
      // NOTE: This tests application-level async handling patterns, not EventBus queuing.
      // EventBus fires all handlers immediately when events are emitted - there is no
      // built-in queue. This test verifies that handlers can implement their own async
      // processing logic without losing events.
      const processedEvents: string[] = [];
      let isProcessing = false;

      eventBus.subscribe('node:updated', async (event: NodeUpdatedEvent) => {
        if (isProcessing) {
          // Handler tracks that it received an event while processing another
          processedEvents.push(`queued-${event.nodeId}`);
        } else {
          isProcessing = true;
          processedEvents.push(`processing-${event.nodeId}`);

          // Simulate batch processing delay
          await new Promise((resolve) => setTimeout(resolve, BATCH_PROCESSING_DELAY));

          isProcessing = false;
        }
      });

      // Fire multiple events rapidly - EventBus calls handler immediately for each
      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'test',
        nodeId: 'node-1',
        updateType: 'content',
        newValue: 'value-1'
      });

      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'test',
        nodeId: 'node-2',
        updateType: 'content',
        newValue: 'value-2'
      });

      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'test',
        nodeId: 'node-3',
        updateType: 'content',
        newValue: 'value-3'
      });

      await waitForEffects();
      await new Promise((resolve) => setTimeout(resolve, BATCH_COMPLETION_WAIT));

      // All events should trigger the handler (EventBus doesn't drop events)
      expect(processedEvents.length).toBeGreaterThanOrEqual(3);
      expect(processedEvents.some((e) => e.includes('node-1'))).toBe(true);
      expect(processedEvents.some((e) => e.includes('node-2'))).toBe(true);
      expect(processedEvents.some((e) => e.includes('node-3'))).toBe(true);
    });
  });
});
