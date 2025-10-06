/**
 * EventBus Multi-Service Coordination Integration Test Suite
 *
 * Tests multi-service event flows and coordination:
 * - Multiple subscribers receiving and filtering events
 * - Service coordination (NodeManager, ContentProcessor, DecorationCoordinator)
 * - Event filtering across namespaces
 * - Error handling for failed/async subscribers
 */

// Mock Svelte 5 runes using shared test utility
import '../setup-svelte-mocks';

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  createReactiveNodeService,
  type ReactiveNodeService as NodeManager,
  type NodeManagerEvents
} from '../../lib/services/reactiveNodeService.svelte.js';
import { eventBus } from '../../lib/services/eventBus';
import { DecorationCoordinator } from '../../lib/services/decorationCoordinator';
import type {
  NodeSpaceEvent,
  CacheInvalidateEvent,
  DecorationClickedEvent,
  DecorationUpdateNeededEvent,
  NodeCreatedEvent,
  NodeUpdatedEvent,
  NodeStatusChangedEvent
} from '../../lib/services/eventTypes';
import {
  isNodeCreatedEvent,
  isNodeUpdatedEvent,
  isDecorationUpdateNeededEvent,
  isReferencesUpdateNeededEvent,
  isCacheInvalidateEvent,
  isNodeStatusChangedEvent,
  isDecorationClickedEvent,
  isHierarchyChangedEvent
} from '../utils/type-guards';
import { waitForEffects, createTestNode } from '../helpers';
import {
  ASYNC_HANDLER_TIMEOUT_MS,
  ASYNC_ERROR_PROPAGATION_TIMEOUT_MS
} from '../utils/test-constants';

describe('EventBus Multi-Service Coordination', () => {
  let nodeManager: NodeManager;
  let mockEvents: NodeManagerEvents;
  let eventLog: NodeSpaceEvent[] = [];

  beforeEach(() => {
    eventLog.length = 0;

    mockEvents = {
      focusRequested: vi.fn(),
      hierarchyChanged: vi.fn(),
      nodeCreated: vi.fn(),
      nodeDeleted: vi.fn()
    };

    nodeManager = createReactiveNodeService(mockEvents);
    DecorationCoordinator.getInstance();

    eventBus.reset();

    // Set up event logging for testing
    eventBus.subscribe('*', (event) => {
      eventLog.push(event);
    });
  });

  afterEach(() => {
    eventBus.reset();
    eventLog = [];
  });

  // ========================================================================
  // Multi-Subscriber Scenarios
  // ========================================================================

  describe('Multi-Subscriber Scenarios', () => {
    it('should trigger multiple subscribers for single event', () => {
      const handler1 = vi.fn();
      const handler2 = vi.fn();
      const handler3 = vi.fn();

      // Initialize with root node first
      nodeManager.initializeNodes([createTestNode('root-1', 'Root node')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Subscribe after initialization
      eventBus.subscribe('node:created', handler1);
      eventBus.subscribe('node:created', handler2);
      eventBus.subscribe('node:created', handler3);

      // Create a new node - this will emit node:created event
      const newNodeId = nodeManager.createNode('root-1', 'Test content');

      // All three handlers should have been called
      expect(handler1).toHaveBeenCalled();
      expect(handler2).toHaveBeenCalled();
      expect(handler3).toHaveBeenCalled();

      // Verify event content using type guards
      const event1 = handler1.mock.calls[0][0];
      const event2 = handler2.mock.calls[0][0];
      const event3 = handler3.mock.calls[0][0];

      expect(isNodeCreatedEvent(event1)).toBe(true);
      expect(isNodeCreatedEvent(event2)).toBe(true);
      expect(isNodeCreatedEvent(event3)).toBe(true);

      if (isNodeCreatedEvent(event1)) {
        expect(event1.nodeId).toBe(newNodeId);
        expect(event1.nodeType).toBe('text');
        expect(event1.source).toBe('ReactiveNodeService');
        expect(event1.namespace).toBe('lifecycle');
      }

      expect(event2).toEqual(event1);
      expect(event3).toEqual(event1);
    });

    it('should deliver correct filtered events to subscribers (integration with NodeManager)', () => {
      const coordinationHandler = vi.fn();
      const interactionHandler = vi.fn();
      const lifecycleHandler = vi.fn();

      // Subscribe with namespace filters
      eventBus.subscribe('*', coordinationHandler, {
        filter: { namespace: 'coordination' }
      });
      eventBus.subscribe('*', interactionHandler, {
        filter: { namespace: 'interaction' }
      });
      eventBus.subscribe('*', lifecycleHandler, {
        filter: { namespace: 'lifecycle' }
      });

      // Initialize node and update content to trigger events
      nodeManager.initializeNodes([createTestNode('node-1', 'Original')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      nodeManager.updateNodeContent('node-1', 'Updated content');

      // Verify each handler only received events from its namespace
      expect(coordinationHandler).toHaveBeenCalled();
      expect(interactionHandler).toHaveBeenCalled();
      expect(lifecycleHandler).toHaveBeenCalled();

      // Verify namespaces are correct
      const coordEvents = coordinationHandler.mock.calls.map((call) => call[0]);
      const interactEvents = interactionHandler.mock.calls.map((call) => call[0]);
      const lifecycleEvents = lifecycleHandler.mock.calls.map((call) => call[0]);

      expect(coordEvents.every((e) => e.namespace === 'coordination')).toBe(true);
      expect(interactEvents.every((e) => e.namespace === 'interaction')).toBe(true);
      expect(lifecycleEvents.every((e) => e.namespace === 'lifecycle')).toBe(true);
    });

    it('should isolate events by namespace', () => {
      const coordinationEvents: NodeSpaceEvent[] = [];
      const interactionEvents: NodeSpaceEvent[] = [];

      eventBus.subscribe(
        '*',
        (e) => {
          coordinationEvents.push(e);
        },
        {
          filter: { namespace: 'coordination' }
        }
      );
      eventBus.subscribe(
        '*',
        (e) => {
          interactionEvents.push(e);
        },
        {
          filter: { namespace: 'interaction' }
        }
      );

      // Emit coordination event
      eventBus.emit<CacheInvalidateEvent>({
        type: 'cache:invalidate',
        namespace: 'coordination',
        source: 'Test',
        cacheKey: 'test-key',
        scope: 'single',
        reason: 'test'
      });

      // Emit interaction event
      eventBus.emit<DecorationUpdateNeededEvent>({
        type: 'decoration:update-needed',
        namespace: 'interaction',
        source: 'Test',
        nodeId: 'test-node',
        decorationType: 'reference',
        reason: 'content-changed'
      });

      // Verify isolation using type guards
      expect(coordinationEvents.length).toBe(1);
      expect(isCacheInvalidateEvent(coordinationEvents[0])).toBe(true);
      if (isCacheInvalidateEvent(coordinationEvents[0])) {
        expect(coordinationEvents[0].namespace).toBe('coordination');
      }

      expect(interactionEvents.length).toBe(1);
      expect(isDecorationUpdateNeededEvent(interactionEvents[0])).toBe(true);
      if (isDecorationUpdateNeededEvent(interactionEvents[0])) {
        expect(interactionEvents[0].namespace).toBe('interaction');
      }
    });

    it('should deliver all events to wildcard subscribers', () => {
      const wildcardEvents: NodeSpaceEvent[] = [];

      // Initialize node first
      nodeManager.initializeNodes([createTestNode('node-1', 'Test')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Subscribe to wildcard after initialization
      eventBus.subscribe('*', (e) => {
        wildcardEvents.push(e);
      });

      // Update content - this will emit multiple events
      nodeManager.updateNodeContent('node-1', 'Updated content');

      // Wildcard subscriber should receive all events
      expect(wildcardEvents.length).toBeGreaterThan(0);

      // Should include various namespaces
      const namespaces = new Set(wildcardEvents.map((e) => e.namespace));
      expect(namespaces.size).toBeGreaterThan(1);
    });
  });

  // ========================================================================
  // Service Coordination
  // ========================================================================

  describe('Service Coordination', () => {
    it('should coordinate decoration update when node content changes', () => {
      // Set up decoration update spy
      const decorationHandler = vi.fn();
      eventBus.subscribe('decoration:update-needed', decorationHandler);

      // Initialize node
      nodeManager.initializeNodes([createTestNode('node-1', 'Original content')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      // Update node content
      nodeManager.updateNodeContent('node-1', 'New content');

      // Verify decoration update was triggered using type guards
      expect(decorationHandler).toHaveBeenCalled();
      const event = decorationHandler.mock.calls[0][0];
      expect(isDecorationUpdateNeededEvent(event)).toBe(true);
      if (isDecorationUpdateNeededEvent(event)) {
        expect(event.nodeId).toBe('node-1');
        expect(event.reason).toBe('content-changed');
        expect(event.namespace).toBe('interaction');
      }
    });

    it('should trigger cache invalidation on node deletion', () => {
      const cacheHandler = vi.fn();
      eventBus.subscribe('cache:invalidate', cacheHandler);

      // Initialize nodes
      nodeManager.initializeNodes(
        [
          createTestNode('parent-1', 'Parent'),
          createTestNode('child-1', 'Child', 'text', 'parent-1')
        ],
        {
          inheritHeaderLevel: 0,
          expanded: true,
          autoFocus: false
        }
      );

      cacheHandler.mockClear();

      // Delete node
      nodeManager.deleteNode('child-1');

      // Verify cache invalidation was triggered using type guards
      expect(cacheHandler).toHaveBeenCalled();
      const cacheEvents = cacheHandler.mock.calls.map((call) => call[0]);
      const deletionEvent = cacheEvents.find(
        (e) => isCacheInvalidateEvent(e) && e.reason === 'node-deleted'
      );
      expect(deletionEvent).toBeDefined();
      if (deletionEvent && isCacheInvalidateEvent(deletionEvent)) {
        expect(deletionEvent.type).toBe('cache:invalidate');
        expect(deletionEvent.namespace).toBe('coordination');
      }
    });

    it('should notify all watchers on status change', () => {
      const statusHandler1 = vi.fn();
      const statusHandler2 = vi.fn();
      const statusHandler3 = vi.fn();

      eventBus.subscribe('node:status-changed', statusHandler1);
      eventBus.subscribe('node:status-changed', statusHandler2);
      eventBus.subscribe('node:status-changed', statusHandler3);

      // Emit status change event
      eventBus.emit<NodeStatusChangedEvent>({
        type: 'node:status-changed',
        namespace: 'coordination',
        source: 'Test',
        nodeId: 'node-1',
        status: 'collapsed',
        previousStatus: 'active'
      });

      // All three watchers should be notified with correct event using type guards
      expect(statusHandler1).toHaveBeenCalledTimes(1);
      expect(statusHandler2).toHaveBeenCalledTimes(1);
      expect(statusHandler3).toHaveBeenCalledTimes(1);

      const event1 = statusHandler1.mock.calls[0][0];
      expect(isNodeStatusChangedEvent(event1)).toBe(true);
      if (isNodeStatusChangedEvent(event1)) {
        expect(event1.nodeId).toBe('node-1');
        expect(event1.status).toBe('collapsed');
        expect(event1.previousStatus).toBe('active');
        expect(event1.namespace).toBe('coordination');
      }
    });

    it('should cascade reference updates correctly', () => {
      const referenceHandler = vi.fn();
      eventBus.subscribe('references:update-needed', referenceHandler);

      // Initialize node
      nodeManager.initializeNodes([createTestNode('node-1', 'Original content')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      referenceHandler.mockClear();

      // Update content - should trigger reference update
      nodeManager.updateNodeContent('node-1', 'Updated content with [[reference]]');

      // Verify reference update event was emitted using type guards
      expect(referenceHandler).toHaveBeenCalled();
      const event = referenceHandler.mock.calls[0][0];
      expect(isReferencesUpdateNeededEvent(event)).toBe(true);
      if (isReferencesUpdateNeededEvent(event)) {
        expect(event.nodeId).toBe('node-1');
        expect(event.updateType).toBe('content');
        expect(event.namespace).toBe('coordination');
      }
    });

    it('should propagate hierarchy changes', () => {
      const hierarchyHandler = vi.fn();
      eventBus.subscribe('hierarchy:changed', hierarchyHandler);

      // Initialize nodes
      nodeManager.initializeNodes([createTestNode('node-1', 'Node 1')], {
        inheritHeaderLevel: 0,
        expanded: true,
        autoFocus: false
      });

      hierarchyHandler.mockClear();

      // Create child node - changes hierarchy
      nodeManager.createNode('node-1', 'Child node');

      // Verify hierarchy change event was emitted using type guards
      expect(hierarchyHandler).toHaveBeenCalled();
      const event = hierarchyHandler.mock.calls[0][0];
      expect(isHierarchyChangedEvent(event)).toBe(true);
      if (isHierarchyChangedEvent(event)) {
        expect(event.type).toBe('hierarchy:changed');
        expect(event.namespace).toBe('lifecycle');
      }
    });
  });

  // ========================================================================
  // Event Filtering
  // ========================================================================

  describe('Event Filtering', () => {
    it('should filter events by namespace (direct emit)', () => {
      const coordinationEvents: NodeSpaceEvent[] = [];
      const interactionEvents: NodeSpaceEvent[] = [];
      const lifecycleEvents: NodeSpaceEvent[] = [];

      eventBus.subscribe(
        '*',
        (e) => {
          coordinationEvents.push(e);
        },
        {
          filter: { namespace: 'coordination' }
        }
      );
      eventBus.subscribe(
        '*',
        (e) => {
          interactionEvents.push(e);
        },
        {
          filter: { namespace: 'interaction' }
        }
      );
      eventBus.subscribe(
        '*',
        (e) => {
          lifecycleEvents.push(e);
        },
        {
          filter: { namespace: 'lifecycle' }
        }
      );

      // Emit events from different namespaces
      eventBus.emit<CacheInvalidateEvent>({
        type: 'cache:invalidate',
        namespace: 'coordination',
        source: 'Test',
        cacheKey: 'test',
        scope: 'single',
        reason: 'test'
      });

      eventBus.emit<DecorationClickedEvent>({
        type: 'decoration:clicked',
        namespace: 'interaction',
        source: 'Test',
        nodeId: 'test',
        decorationType: 'reference',
        target: 'target',
        clickPosition: { x: 0, y: 0 }
      });

      eventBus.emit<NodeCreatedEvent>({
        type: 'node:created',
        namespace: 'lifecycle',
        source: 'Test',
        nodeId: 'test',
        nodeType: 'text'
      });

      // Verify filtering using type guards
      expect(coordinationEvents.length).toBe(1);
      expect(isCacheInvalidateEvent(coordinationEvents[0])).toBe(true);
      if (isCacheInvalidateEvent(coordinationEvents[0])) {
        expect(coordinationEvents[0].namespace).toBe('coordination');
      }

      expect(interactionEvents.length).toBe(1);
      expect(isDecorationClickedEvent(interactionEvents[0])).toBe(true);
      if (isDecorationClickedEvent(interactionEvents[0])) {
        expect(interactionEvents[0].namespace).toBe('interaction');
      }

      expect(lifecycleEvents.length).toBe(1);
      expect(isNodeCreatedEvent(lifecycleEvents[0])).toBe(true);
      if (isNodeCreatedEvent(lifecycleEvents[0])) {
        expect(lifecycleEvents[0].namespace).toBe('lifecycle');
      }
    });

    it('should filter events by source', () => {
      const nodeManagerEvents: NodeSpaceEvent[] = [];
      const otherEvents: NodeSpaceEvent[] = [];

      eventBus.subscribe(
        '*',
        (e) => {
          nodeManagerEvents.push(e);
        },
        {
          filter: { source: 'ReactiveNodeService' }
        }
      );
      eventBus.subscribe(
        '*',
        (e) => {
          otherEvents.push(e);
        },
        {
          filter: { source: 'OtherSource' }
        }
      );

      // Emit events from different sources
      eventBus.emit<NodeCreatedEvent>({
        type: 'node:created',
        namespace: 'lifecycle',
        source: 'ReactiveNodeService',
        nodeId: 'test1',
        nodeType: 'text'
      });

      eventBus.emit<NodeCreatedEvent>({
        type: 'node:created',
        namespace: 'lifecycle',
        source: 'OtherSource',
        nodeId: 'test2',
        nodeType: 'text'
      });

      // Verify source filtering using type guards
      expect(nodeManagerEvents.length).toBe(1);
      expect(isNodeCreatedEvent(nodeManagerEvents[0])).toBe(true);
      if (isNodeCreatedEvent(nodeManagerEvents[0])) {
        expect(nodeManagerEvents[0].source).toBe('ReactiveNodeService');
      }

      expect(otherEvents.length).toBe(1);
      expect(isNodeCreatedEvent(otherEvents[0])).toBe(true);
      if (isNodeCreatedEvent(otherEvents[0])) {
        expect(otherEvents[0].source).toBe('OtherSource');
      }
    });

    it('should filter events by nodeId', () => {
      const node1Events: NodeSpaceEvent[] = [];
      const node2Events: NodeSpaceEvent[] = [];

      eventBus.subscribe(
        '*',
        (e) => {
          node1Events.push(e);
        },
        {
          filter: { nodeId: 'node-1' }
        }
      );
      eventBus.subscribe(
        '*',
        (e) => {
          node2Events.push(e);
        },
        {
          filter: { nodeId: 'node-2' }
        }
      );

      // Emit events for different nodes
      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'Test',
        nodeId: 'node-1',
        updateType: 'content',
        newValue: 'updated'
      });

      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'Test',
        nodeId: 'node-2',
        updateType: 'content',
        newValue: 'updated'
      });

      // Verify nodeId filtering using type guards
      expect(node1Events.length).toBe(1);
      expect(isNodeUpdatedEvent(node1Events[0])).toBe(true);
      if (isNodeUpdatedEvent(node1Events[0])) {
        expect(node1Events[0].nodeId).toBe('node-1');
      }

      expect(node2Events.length).toBe(1);
      expect(isNodeUpdatedEvent(node2Events[0])).toBe(true);
      if (isNodeUpdatedEvent(node2Events[0])) {
        expect(node2Events[0].nodeId).toBe('node-2');
      }
    });
  });

  // ========================================================================
  // Error Handling
  // ========================================================================

  describe('Error Handling', () => {
    it('should continue calling other subscribers after handler failure', () => {
      const errorHandler = vi.fn(() => {
        throw new Error('Handler failed');
      });
      const successHandler = vi.fn();

      // Error handler is subscribed first
      eventBus.subscribe('node:created', errorHandler);
      eventBus.subscribe('node:created', successHandler);

      // Suppress console.error for this test
      const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

      // Emit event
      eventBus.emit<NodeCreatedEvent>({
        type: 'node:created',
        namespace: 'lifecycle',
        source: 'Test',
        nodeId: 'test',
        nodeType: 'text'
      });

      // Both handlers should have been called with correct event using type guards
      expect(errorHandler).toHaveBeenCalled();
      expect(successHandler).toHaveBeenCalled();

      const event = successHandler.mock.calls[0][0];
      expect(isNodeCreatedEvent(event)).toBe(true);
      if (isNodeCreatedEvent(event)) {
        expect(event.nodeId).toBe('test');
        expect(event.nodeType).toBe('text');
      }

      // Error should have been logged
      expect(consoleErrorSpy).toHaveBeenCalledWith(
        'EventBus: Error in event handler',
        expect.objectContaining({
          eventType: 'node:created'
        })
      );

      consoleErrorSpy.mockRestore();
    });

    it('should handle async subscriber errors gracefully', async () => {
      const asyncErrorHandler = vi.fn(async (_event: NodeUpdatedEvent) => {
        throw new Error('Async handler failed');
      });
      const asyncSuccessHandler = vi.fn(async (_event: NodeUpdatedEvent) => {
        await waitForEffects(ASYNC_HANDLER_TIMEOUT_MS);
      });

      eventBus.subscribe('node:updated', asyncErrorHandler);
      eventBus.subscribe('node:updated', asyncSuccessHandler);

      const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

      eventBus.emit<NodeUpdatedEvent>({
        type: 'node:updated',
        namespace: 'lifecycle',
        source: 'Test',
        nodeId: 'test',
        updateType: 'content',
        newValue: 'updated'
      });

      // Wait for async handlers to complete and errors to propagate
      await waitForEffects(ASYNC_ERROR_PROPAGATION_TIMEOUT_MS);

      // Both handlers should have been called with correct event using type guards
      expect(asyncErrorHandler).toHaveBeenCalled();
      expect(asyncSuccessHandler).toHaveBeenCalled();

      const event = asyncSuccessHandler.mock.calls[0][0];
      expect(isNodeUpdatedEvent(event)).toBe(true);
      if (isNodeUpdatedEvent(event)) {
        expect(event.nodeId).toBe('test');
        expect(event.updateType).toBe('content');
      }

      // Async error should have been logged
      expect(consoleErrorSpy).toHaveBeenCalledWith(
        'EventBus: Async handler error',
        expect.objectContaining({
          eventType: 'node:updated'
        })
      );

      consoleErrorSpy.mockRestore();
    });
  });
});
