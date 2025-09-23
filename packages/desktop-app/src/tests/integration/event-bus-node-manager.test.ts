/**
 * EventBus-NodeManager Integration Test Suite
 *
 * Tests the integration between EventBus and NodeManager for:
 * - Real-time status updates across node references
 * - Cache invalidation coordination
 * - Dynamic decoration updates
 * - Performance under high-frequency operations
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  createReactiveNodeService,
  ReactiveNodeService as NodeManager
} from '../../lib/services/reactiveNodeService.svelte.js';
import { eventBus } from '../../lib/services/eventBus';
import type { NodeManagerEvents } from '../../lib/services/reactiveNodeService.svelte.js';
import type { NodeSpaceEvent } from '../../lib/services/eventTypes';

describe('EventBus-NodeManager Integration', () => {
  let nodeManager: NodeManager;
  let mockEvents: NodeManagerEvents;
  let eventLog: NodeSpaceEvent[] = [];

  beforeEach(() => {
    eventLog = [];

    // Set up mock events
    mockEvents = {
      focusRequested: vi.fn(),
      hierarchyChanged: vi.fn(),
      nodeCreated: vi.fn(),
      nodeDeleted: vi.fn()
    };

    nodeManager = createReactiveNodeService(mockEvents);
    // Skip ReactiveNodeManager in tests due to $state not being available

    // Reset EventBus state first
    eventBus.reset();

    // Set up event logging for testing
    eventBus.subscribe('*', (event) => {
      eventLog.push(event);
    });
  });

  afterEach(() => {
    eventBus.reset();
  });

  // ========================================================================
  // Basic Integration Tests
  // ========================================================================

  describe('Basic Integration', () => {
    it('should emit events when nodes are created', () => {
      // Create initial node data
      nodeManager.initializeFromLegacyData([
        { id: 'root1', content: 'Root node', type: 'text', children: [] }
      ]);

      // Clear initial events
      eventLog.length = 0;

      // Create a new node
      const newNodeId = nodeManager.createNode('root1', 'New node content');

      // Check that creation events were emitted
      const createdEvents = eventLog.filter((e) => e.type === 'node:created');
      expect(createdEvents).toHaveLength(1);
      expect(createdEvents[0].nodeId).toBe(newNodeId);

      const hierarchyEvents = eventLog.filter((e) => e.type === 'hierarchy:changed');
      expect(hierarchyEvents).toHaveLength(1);

      const cacheEvents = eventLog.filter((e) => e.type === 'cache:invalidate');
      expect(cacheEvents.length).toBeGreaterThanOrEqual(1);
    });

    it('should emit events when node content is updated', () => {
      // Initialize with test data
      nodeManager.initializeFromLegacyData([
        { id: 'node1', content: 'Original content', type: 'text', children: [] }
      ]);

      eventLog.length = 0;

      // Update node content
      nodeManager.updateNodeContent('node1', 'Updated content');

      // Check events
      const updatedEvents = eventLog.filter((e) => e.type === 'node:updated');
      expect(updatedEvents).toHaveLength(1);
      expect(updatedEvents[0].nodeId).toBe('node1');
      expect(updatedEvents[0].updateType).toBe('content');

      const decorationEvents = eventLog.filter((e) => e.type === 'decoration:update-needed');
      expect(decorationEvents).toHaveLength(1);

      const referenceEvents = eventLog.filter((e) => e.type === 'references:update-needed');
      expect(referenceEvents).toHaveLength(1);

      const cacheEvents = eventLog.filter((e) => e.type === 'cache:invalidate');
      expect(cacheEvents).toHaveLength(1);
    });

    it('should emit events when nodes are deleted', () => {
      // Initialize with test data
      nodeManager.initializeFromLegacyData([
        {
          id: 'parent1',
          content: 'Parent node',
          type: 'text',
          children: [{ id: 'child1', content: 'Child node', type: 'text', children: [] }]
        }
      ]);

      eventLog.length = 0;

      // Delete child node
      nodeManager.deleteNode('child1');

      // Check events
      const deletedEvents = eventLog.filter((e) => e.type === 'node:deleted');
      expect(deletedEvents).toHaveLength(1);
      expect(deletedEvents[0].nodeId).toBe('child1');

      const hierarchyEvents = eventLog.filter((e) => e.type === 'hierarchy:changed');
      expect(hierarchyEvents).toHaveLength(1);

      const referenceEvents = eventLog.filter((e) => e.type === 'references:update-needed');
      expect(referenceEvents).toHaveLength(1);
      expect(referenceEvents[0].updateType).toBe('deletion');

      const cacheEvents = eventLog.filter((e) => e.type === 'cache:invalidate');
      expect(cacheEvents.length).toBeGreaterThanOrEqual(1);
    });
  });

  // ========================================================================
  // Status Change Coordination Tests
  // ========================================================================

  describe('Status Change Coordination', () => {
    it('should coordinate node status changes', () => {
      // Initialize with test data
      nodeManager.initializeFromLegacyData([
        { id: 'node1', content: 'Test node', type: 'text', children: [] }
      ]);

      eventLog.length = 0;

      // Set active node
      nodeManager.setActiveNode('node1');

      // Check status change events
      const statusEvents = eventLog.filter((e) => e.type === 'node:status-changed');
      expect(statusEvents).toHaveLength(1);
      expect(statusEvents[0].nodeId).toBe('node1');
      expect(statusEvents[0].status).toBe('focused');
    });

    it('should handle expanded/collapsed status changes', () => {
      // Initialize with test data
      nodeManager.initializeFromLegacyData([
        {
          id: 'node1',
          content: 'Test node',
          type: 'text',
          expanded: true,
          children: []
        }
      ]);

      eventLog.length = 0;

      // Toggle expanded state
      nodeManager.toggleExpanded('node1');

      // Check events
      const statusEvents = eventLog.filter((e) => e.type === 'node:status-changed');
      expect(statusEvents).toHaveLength(1);
      expect(statusEvents[0].status).toBe('collapsed');

      const hierarchyEvents = eventLog.filter((e) => e.type === 'hierarchy:changed');
      expect(hierarchyEvents).toHaveLength(1);
      expect(hierarchyEvents[0].changeType).toBe('collapse');
    });
  });

  // ========================================================================
  // Cache Invalidation Coordination Tests
  // ========================================================================

  describe('Cache Invalidation Coordination', () => {
    it('should emit cache invalidation events on node operations', () => {
      // Initialize with test data
      nodeManager.initializeFromLegacyData([
        { id: 'node1', content: 'Test node', type: 'text', children: [] }
      ]);

      eventLog.length = 0;

      // Perform operations that should invalidate cache
      nodeManager.updateNodeContent('node1', 'Updated content');

      const cacheEvents = eventLog.filter((e) => e.type === 'cache:invalidate');
      expect(cacheEvents.length).toBeGreaterThanOrEqual(1);

      const nodeCacheEvent = cacheEvents.find((e) => e.scope === 'node' && e.nodeId === 'node1');
      expect(nodeCacheEvent).toBeDefined();
    });

    it('should handle global cache invalidation on deletions', () => {
      // Initialize with test data
      nodeManager.initializeFromLegacyData([
        { id: 'node1', content: 'Test node', type: 'text', children: [] }
      ]);

      eventLog.length = 0;

      // Delete node (should cause global cache invalidation)
      nodeManager.deleteNode('node1');

      const globalCacheEvents = eventLog.filter(
        (e) => e.type === 'cache:invalidate' && e.scope === 'global'
      );
      expect(globalCacheEvents.length).toBeGreaterThanOrEqual(1);
    });
  });

  // ========================================================================
  // Hierarchy Change Coordination Tests
  // ========================================================================

  describe('Hierarchy Change Coordination', () => {
    it('should coordinate indent operations', () => {
      // Initialize with test data
      nodeManager.initializeFromLegacyData([
        { id: 'node1', content: 'Node 1', type: 'text', children: [] },
        { id: 'node2', content: 'Node 2', type: 'text', children: [] }
      ]);

      eventLog.length = 0;

      // Indent node2 under node1
      const result = nodeManager.indentNode('node2');
      expect(result).toBe(true);

      // Check hierarchy change events
      const hierarchyEvents = eventLog.filter((e) => e.type === 'hierarchy:changed');
      expect(hierarchyEvents).toHaveLength(1);
      expect(hierarchyEvents[0].changeType).toBe('indent');
      expect(hierarchyEvents[0].affectedNodes).toContain('node2');

      const referenceEvents = eventLog.filter((e) => e.type === 'references:update-needed');
      expect(referenceEvents).toHaveLength(1);
      expect(referenceEvents[0].updateType).toBe('hierarchy');
    });

    it('should coordinate outdent operations', () => {
      // Initialize with nested data
      nodeManager.initializeFromLegacyData([
        {
          id: 'parent',
          content: 'Parent',
          type: 'text',
          children: [{ id: 'child', content: 'Child', type: 'text', children: [] }]
        }
      ]);

      eventLog.length = 0;

      // Outdent child node
      const result = nodeManager.outdentNode('child');
      expect(result).toBe(true);

      // Check events
      const hierarchyEvents = eventLog.filter((e) => e.type === 'hierarchy:changed');
      expect(hierarchyEvents).toHaveLength(1);
      expect(hierarchyEvents[0].changeType).toBe('outdent');
    });
  });

  // ========================================================================
  // Reactive Manager Integration Tests (Skipped - requires Svelte runtime)
  // ========================================================================

  describe('Reactive Manager Integration', () => {
    it.skip('should update reactive state on EventBus events', () => {
      // Skipped due to $state not being available in test environment
    });

    it.skip('should coordinate between EventBus and reactive state', () => {
      // Skipped due to $state not being available in test environment
    });
  });

  // ========================================================================
  // Performance Tests
  // ========================================================================

  describe('Performance Integration', () => {
    it('should handle high-frequency node updates efficiently', () => {
      // Initialize with multiple nodes
      const testNodes = Array.from({ length: 10 }, (_, i) => ({
        id: `node${i}`,
        content: `Node ${i}`,
        type: 'text',
        children: []
      }));

      nodeManager.initializeFromLegacyData(testNodes);
      eventLog.length = 0;

      const startTime = performance.now();

      // Perform many updates
      for (let i = 0; i < 10; i++) {
        nodeManager.updateNodeContent(`node${i}`, `Updated content ${i}`);
      }

      const endTime = performance.now();
      const duration = endTime - startTime;

      // Should complete quickly (less than 100ms for 10 operations)
      expect(duration).toBeLessThan(100);

      // Should have emitted appropriate events
      const updatedEvents = eventLog.filter((e) => e.type === 'node:updated');
      expect(updatedEvents).toHaveLength(10);

      // EventBus should maintain performance metrics
      const metrics = eventBus.getMetrics();
      expect(metrics.totalEvents).toBeGreaterThanOrEqual(30); // At least 3 events per update
      expect(metrics.averageProcessingTime).toBeDefined();
    });

    it('should batch similar events when configured', async () => {
      // Configure batching for status change events
      eventBus.configureBatching({
        maxBatchSize: 5,
        timeWindowMs: 50,
        enableForTypes: ['node:status-changed']
      });

      // Initialize nodes
      const testNodes = Array.from({ length: 5 }, (_, i) => ({
        id: `node${i}`,
        content: `Node ${i}`,
        type: 'text',
        children: []
      }));

      nodeManager.initializeFromLegacyData(testNodes);
      eventLog.length = 0;

      // Set multiple nodes as active quickly
      for (let i = 0; i < 5; i++) {
        nodeManager.setActiveNode(`node${i}`);
      }

      // Wait for batch processing
      await new Promise((resolve) => setTimeout(resolve, 100));

      // Check that events were processed (exact timing depends on batching)
      const statusEvents = eventLog.filter((e) => e.type === 'node:status-changed');
      expect(statusEvents.length).toBeGreaterThanOrEqual(5);
    });
  });

  // ========================================================================
  // Error Recovery Tests
  // ========================================================================

  describe('Error Recovery Integration', () => {
    it('should continue functioning after event handler errors', () => {
      // Set up a faulty event handler
      const faultyHandler = vi.fn(() => {
        throw new Error('Simulated error');
      });

      eventBus.subscribe('node:updated', faultyHandler);

      // Also set up a normal handler
      const normalHandler = vi.fn();
      eventBus.subscribe('node:updated', normalHandler);

      // Initialize node
      nodeManager.initializeFromLegacyData([
        { id: 'node1', content: 'Test node', type: 'text', children: [] }
      ]);

      eventLog.length = 0;

      // Update content (should trigger error in faulty handler)
      expect(() => {
        nodeManager.updateNodeContent('node1', 'Updated content');
      }).not.toThrow();

      // Normal handler should still work
      expect(normalHandler).toHaveBeenCalled();

      // EventBus should continue to function
      const updatedEvents = eventLog.filter((e) => e.type === 'node:updated');
      expect(updatedEvents).toHaveLength(1);
    });

    it('should maintain consistency after network-like failures', () => {
      // Simulate intermittent failures by temporarily disabling EventBus
      nodeManager.initializeFromLegacyData([
        { id: 'node1', content: 'Test node', type: 'text', children: [] }
      ]);

      eventLog.length = 0;

      // Disable EventBus temporarily
      eventBus.setEnabled(false);

      // Perform operations (should not emit events)
      nodeManager.updateNodeContent('node1', 'Content during outage');
      expect(eventLog).toHaveLength(0);

      // Re-enable EventBus
      eventBus.setEnabled(true);

      // Subsequent operations should work normally
      nodeManager.updateNodeContent('node1', 'Content after recovery');

      const updatedEvents = eventLog.filter((e) => e.type === 'node:updated');
      expect(updatedEvents).toHaveLength(1);
      expect(updatedEvents[0].newValue).toBe('Content after recovery');
    });
  });
});
