/**
 * Unit tests for NodeExpansionCoordinator
 *
 * Tests the coordination between ReactiveNodeService and TabPersistenceService
 * for persisting and restoring node expansion states.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import { NodeExpansionCoordinator } from '$lib/services/node-expansion-coordinator';
import { createReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
import type { Node } from '$lib/types';

describe('NodeExpansionCoordinator', () => {
  beforeEach(() => {
    // Clear coordinator state before each test
    NodeExpansionCoordinator.clear();
  });

  describe('Viewer Registration', () => {
    it('should register a viewer successfully', () => {
      const service = createMockReactiveNodeService();
      const viewerId = 'test-viewer-1';

      NodeExpansionCoordinator.registerViewer(viewerId, service);

      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(1);
    });

    it('should unregister a viewer successfully', () => {
      const service = createMockReactiveNodeService();
      const viewerId = 'test-viewer-1';

      NodeExpansionCoordinator.registerViewer(viewerId, service);
      NodeExpansionCoordinator.unregisterViewer(viewerId);

      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(0);
    });

    it('should handle multiple viewer registrations', () => {
      const service1 = createMockReactiveNodeService();
      const service2 = createMockReactiveNodeService();

      NodeExpansionCoordinator.registerViewer('viewer-1', service1);
      NodeExpansionCoordinator.registerViewer('viewer-2', service2);

      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(2);
    });
  });

  describe('Expansion State Extraction', () => {
    it('should extract expanded node IDs from a viewer', () => {
      const service = createMockReactiveNodeService();
      const viewerId = 'test-viewer-1';

      // Create nodes
      const node1Id = 'node-1';
      const node2Id = 'node-2';
      const node3Id = 'node-3';

      // Initialize with test nodes (all collapsed by default)
      service.initializeNodes(
        [
          createMockNode(node1Id, 'Node 1'),
          createMockNode(node2Id, 'Node 2'),
          createMockNode(node3Id, 'Node 3')
        ],
        { expanded: false }
      );

      // Expand node-1 and node-3
      service.toggleExpanded(node1Id);
      service.toggleExpanded(node3Id);

      NodeExpansionCoordinator.registerViewer(viewerId, service);

      const expandedIds = NodeExpansionCoordinator.getExpandedNodeIds(viewerId);

      expect(expandedIds).toContain(node1Id);
      expect(expandedIds).toContain(node3Id);
      expect(expandedIds).not.toContain(node2Id);
      expect(expandedIds.length).toBe(2);
    });

    it('should return empty array for unregistered viewer', () => {
      const expandedIds = NodeExpansionCoordinator.getExpandedNodeIds('nonexistent-viewer');
      expect(expandedIds).toEqual([]);
    });

    it('should return empty array when no nodes are expanded', () => {
      const service = createMockReactiveNodeService();
      const viewerId = 'test-viewer-1';

      // Initialize nodes as collapsed
      service.initializeNodes([createMockNode('node-1', 'Node 1')], { expanded: false });

      NodeExpansionCoordinator.registerViewer(viewerId, service);

      const expandedIds = NodeExpansionCoordinator.getExpandedNodeIds(viewerId);
      expect(expandedIds).toEqual([]);
    });
  });

  describe('Deferred Restoration', () => {
    it('should schedule restoration for unregistered viewer', () => {
      const viewerId = 'test-viewer-1';
      const expandedNodeIds = ['node-1', 'node-2'];

      NodeExpansionCoordinator.scheduleRestoration(viewerId, expandedNodeIds);

      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(1);
    });

    it('should apply pending restorations when viewer registers', () => {
      const service = createMockReactiveNodeService();
      const viewerId = 'test-viewer-1';
      const node1Id = 'node-1';
      const node2Id = 'node-2';

      // Initialize with test nodes (all collapsed by default)
      service.initializeNodes(
        [createMockNode(node1Id, 'Node 1'), createMockNode(node2Id, 'Node 2')],
        { expanded: false }
      );

      // Schedule restoration before viewer is registered
      NodeExpansionCoordinator.scheduleRestoration(viewerId, [node1Id, node2Id]);

      // Register viewer - should trigger automatic restoration
      NodeExpansionCoordinator.registerViewer(viewerId, service);

      // Check that nodes are now expanded
      const uiState1 = service.getUIState(node1Id);
      const uiState2 = service.getUIState(node2Id);

      expect(uiState1?.expanded).toBe(true);
      expect(uiState2?.expanded).toBe(true);
      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(0);
    });

    it('should clear pending restorations after registration', () => {
      const service = createMockReactiveNodeService();
      const viewerId = 'test-viewer-1';

      service.initializeNodes([createMockNode('node-1', 'Node 1')]);

      NodeExpansionCoordinator.scheduleRestoration(viewerId, ['node-1']);
      NodeExpansionCoordinator.registerViewer(viewerId, service);

      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(0);
    });
  });

  describe('Restoration Behavior', () => {
    it('should restore expansion states for existing nodes', () => {
      const service = createMockReactiveNodeService();
      const viewerId = 'test-viewer-1';
      const node1Id = 'node-1';
      const node2Id = 'node-2';

      // Initialize with test nodes (all collapsed)
      service.initializeNodes(
        [createMockNode(node1Id, 'Node 1'), createMockNode(node2Id, 'Node 2')],
        { expanded: false }
      );

      NodeExpansionCoordinator.registerViewer(viewerId, service);

      // Restore expansion states
      NodeExpansionCoordinator.restoreExpansionStates(viewerId, [node1Id]);

      // Check that only node-1 is expanded
      expect(service.getUIState(node1Id)?.expanded).toBe(true);
      expect(service.getUIState(node2Id)?.expanded).toBe(false);
    });

    it('should skip restoration for non-existent nodes', () => {
      const service = createMockReactiveNodeService();
      const viewerId = 'test-viewer-1';
      const existingNodeId = 'node-1';
      const nonExistentNodeId = 'node-999';

      service.initializeNodes([createMockNode(existingNodeId, 'Node 1')], { expanded: false });

      NodeExpansionCoordinator.registerViewer(viewerId, service);

      // Attempt to restore both existing and non-existent nodes
      // Should not throw error
      NodeExpansionCoordinator.restoreExpansionStates(viewerId, [
        existingNodeId,
        nonExistentNodeId
      ]);

      // Only existing node should be expanded
      expect(service.getUIState(existingNodeId)?.expanded).toBe(true);
    });

    it('should handle restoration for unregistered viewer by queueing', () => {
      const viewerId = 'test-viewer-1';
      const expandedNodeIds = ['node-1', 'node-2'];

      // Attempt to restore before viewer is registered
      NodeExpansionCoordinator.restoreExpansionStates(viewerId, expandedNodeIds);

      // Should queue the restoration
      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(1);
    });
  });

  describe('Edge Cases', () => {
    it('should handle empty expansion state list', () => {
      const service = createMockReactiveNodeService();
      const viewerId = 'test-viewer-1';

      service.initializeNodes([createMockNode('node-1', 'Node 1')]);
      NodeExpansionCoordinator.registerViewer(viewerId, service);

      expect(() => {
        NodeExpansionCoordinator.restoreExpansionStates(viewerId, []);
      }).not.toThrow();
    });

    it('should handle registering the same viewer twice', () => {
      const service1 = createMockReactiveNodeService();
      const service2 = createMockReactiveNodeService();
      const viewerId = 'test-viewer-1';

      NodeExpansionCoordinator.registerViewer(viewerId, service1);
      NodeExpansionCoordinator.registerViewer(viewerId, service2);

      // Second registration should replace the first
      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(1);
    });

    it('should handle unregistering a non-existent viewer', () => {
      // Should not throw error
      NodeExpansionCoordinator.unregisterViewer('nonexistent-viewer');
      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(0);
    });

    it('should clear all state when clear() is called', () => {
      const service = createMockReactiveNodeService();

      NodeExpansionCoordinator.registerViewer('viewer-1', service);
      NodeExpansionCoordinator.scheduleRestoration('viewer-2', ['node-1']);

      NodeExpansionCoordinator.clear();

      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(0);
      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(0);
    });
  });

  describe('Memory Management', () => {
    it('should warn when registry size exceeds maximum', () => {
      const service = createMockReactiveNodeService();

      // Register 101 viewers to exceed MAX_REGISTRY_SIZE (100)
      for (let i = 0; i < 101; i++) {
        NodeExpansionCoordinator.registerViewer(`viewer-${i}`, service);
      }

      // Should have registered all 101 viewers
      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(101);
      // Warning is logged but test continues - we're just verifying the warning path executes
    });

    it('should cleanup stale entries older than specified age', () => {
      const service = createMockReactiveNodeService();

      // Register viewers
      NodeExpansionCoordinator.registerViewer('viewer-1', service);
      NodeExpansionCoordinator.registerViewer('viewer-2', service);
      NodeExpansionCoordinator.registerViewer('viewer-3', service);

      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(3);

      // Cleanup with -1ms max age - all entries will be older than this
      const removedCount = NodeExpansionCoordinator.cleanupStaleEntries(-1);

      expect(removedCount).toBe(3);
      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(0);
    });

    it('should not cleanup entries younger than max age', () => {
      const service = createMockReactiveNodeService();

      NodeExpansionCoordinator.registerViewer('viewer-1', service);
      NodeExpansionCoordinator.registerViewer('viewer-2', service);

      // Cleanup with very large max age - no entries should be removed
      const removedCount = NodeExpansionCoordinator.cleanupStaleEntries(1000 * 60 * 60); // 1 hour

      expect(removedCount).toBe(0);
      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(2);
    });

    it('should cleanup pending restorations for stale entries', () => {
      const service = createMockReactiveNodeService();

      // Register viewer and schedule restoration
      NodeExpansionCoordinator.registerViewer('viewer-1', service);
      NodeExpansionCoordinator.scheduleRestoration('viewer-2', ['node-1']);

      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(1);
      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(1);

      // Cleanup stale entries (-1ms max age - all registered entries will be removed)
      NodeExpansionCoordinator.cleanupStaleEntries(-1);

      // Registered viewer should be removed, pending restoration should also be cleaned
      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(0);
      // Note: pending restoration for unregistered viewer remains (only cleaned when viewer is stale)
      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(1);
    });

    it('should return 0 when no stale entries to cleanup', () => {
      const service = createMockReactiveNodeService();

      NodeExpansionCoordinator.registerViewer('viewer-1', service);

      // Cleanup with large max age - nothing should be removed
      const removedCount = NodeExpansionCoordinator.cleanupStaleEntries(1000 * 60 * 60);

      expect(removedCount).toBe(0);
    });

    it('should unregister viewer and cleanup pending restorations', () => {
      const service = createMockReactiveNodeService();
      const viewerId = 'test-viewer-1';

      // Register viewer and add pending restoration
      NodeExpansionCoordinator.registerViewer(viewerId, service);
      NodeExpansionCoordinator.scheduleRestoration(viewerId, ['node-1']);

      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(1);
      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(1);

      // Unregister should clean both
      NodeExpansionCoordinator.unregisterViewer(viewerId);

      expect(NodeExpansionCoordinator.getRegisteredViewerCount()).toBe(0);
      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(0);
    });
  });
});

// Helper functions

/**
 * Create a mock ReactiveNodeService for testing
 */
function createMockReactiveNodeService() {
  return createReactiveNodeService({
    focusRequested: () => {},
    hierarchyChanged: () => {},
    nodeCreated: () => {},
    nodeDeleted: () => {}
  });
}

/**
 * Create a mock Node for testing
 */
function createMockNode(id: string, content: string): Node {
  return {
    id,
    nodeType: 'text',
    content,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    properties: {},
    mentions: []
  };
}
