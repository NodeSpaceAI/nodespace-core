/**
 * Integration tests for expansion state persistence
 *
 * Tests the complete flow of persisting and restoring node expansion states
 * across application restarts, including TabPersistenceService, NodeExpansionCoordinator,
 * and ReactiveNodeService integration.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { NodeExpansionCoordinator } from '$lib/services/node-expansion-coordinator';
import { TabPersistenceService } from '$lib/services/tab-persistence-service';
import { createReactiveNodeService } from '$lib/services/reactive-node-service.svelte';
import type { Node } from '$lib/types';
import type { TabState } from '$lib/stores/navigation';

describe('Expansion State Persistence Integration', () => {
  beforeEach(() => {
    // Clear all state before each test
    NodeExpansionCoordinator.clear();
    TabPersistenceService.clear();
  });

  describe('End-to-End Persistence Flow', () => {
    it('should persist and restore expansion states across simulated app restart', () => {
      // ==========================================
      // PHASE 1: Initial Setup & User Interaction
      // ==========================================

      const tabId = 'test-tab-1';
      const service = createMockReactiveNodeService();

      // Create a hierarchy of nodes
      const rootId = 'root-1';
      const child1Id = 'child-1';
      const child2Id = 'child-2';
      const grandchild1Id = 'grandchild-1';

      service.initializeNodes(
        [
          createMockNode(rootId, 'Root Node', null),
          createMockNode(child1Id, 'Child 1', rootId),
          createMockNode(child2Id, 'Child 2', rootId),
          createMockNode(grandchild1Id, 'Grandchild 1', child1Id)
        ],
        { expanded: false }
      );

      // Register viewer
      NodeExpansionCoordinator.registerViewer(tabId, service);

      // Simulate user expanding some nodes
      service.toggleExpanded(rootId); // Expand root
      service.toggleExpanded(child1Id); // Expand child1
      // child2 remains collapsed

      // ==========================================
      // PHASE 2: Save State (Simulating App Close)
      // ==========================================

      // Extract expansion states (simulates navigation store subscription)
      const expandedNodeIds = NodeExpansionCoordinator.getExpandedNodeIds(tabId);

      // Create tab state with expansion data
      const tabState: TabState = {
        tabs: [
          {
            id: tabId,
            title: 'Test Tab',
            type: 'node',
            content: { nodeId: rootId },
            closeable: true,
            paneId: 'pane-1',
            expandedNodeIds: expandedNodeIds
          }
        ],
        panes: [
          {
            id: 'pane-1',
            width: 100,
            tabIds: [tabId]
          }
        ],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': tabId }
      };

      // Save to localStorage immediately (bypass debounce for testing)
      TabPersistenceService.saveNow(tabState);

      // Verify expansion state was captured
      expect(expandedNodeIds).toContain(rootId);
      expect(expandedNodeIds).toContain(child1Id);
      expect(expandedNodeIds).not.toContain(child2Id);
      expect(expandedNodeIds).not.toContain(grandchild1Id);

      // ==========================================
      // PHASE 3: Simulate App Restart
      // ==========================================

      // Clear runtime state (simulates app restart)
      NodeExpansionCoordinator.clear();

      // ==========================================
      // PHASE 4: Load Persisted State
      // ==========================================

      // Load tab state from localStorage
      const loadedState = TabPersistenceService.load();
      expect(loadedState).toBeTruthy();
      expect(loadedState?.tabs[0].expandedNodeIds).toEqual(expandedNodeIds);

      // Schedule restoration for each tab (simulates loadPersistedState in navigation.ts)
      if (loadedState) {
        for (const tab of loadedState.tabs) {
          if (tab.expandedNodeIds && tab.expandedNodeIds.length > 0) {
            NodeExpansionCoordinator.scheduleRestoration(tab.id, tab.expandedNodeIds);
          }
        }
      }

      // Verify restoration was scheduled
      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(1);

      // ==========================================
      // PHASE 5: Viewer Re-initialization
      // ==========================================

      // Create new service instance (simulates component remount)
      const newService = createMockReactiveNodeService();

      // Load nodes from database (all collapsed by default)
      newService.initializeNodes(
        [
          createMockNode(rootId, 'Root Node', null),
          createMockNode(child1Id, 'Child 1', rootId),
          createMockNode(child2Id, 'Child 2', rootId),
          createMockNode(grandchild1Id, 'Grandchild 1', child1Id)
        ],
        { expanded: false } // Important: nodes start collapsed
      );

      // Register viewer - should trigger automatic restoration
      NodeExpansionCoordinator.registerViewer(tabId, newService);

      // ==========================================
      // PHASE 6: Verify Restoration
      // ==========================================

      // Check that expansion states were restored
      expect(newService.getUIState(rootId)?.expanded).toBe(true);
      expect(newService.getUIState(child1Id)?.expanded).toBe(true);
      expect(newService.getUIState(child2Id)?.expanded).toBe(false);
      expect(newService.getUIState(grandchild1Id)?.expanded).toBe(false);

      // Pending restorations should be cleared
      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(0);
    });

    it('should handle multiple tabs with independent expansion states', () => {
      const tab1Id = 'tab-1';
      const tab2Id = 'tab-2';

      // Create two independent services
      const service1 = createMockReactiveNodeService();
      const service2 = createMockReactiveNodeService();

      // Initialize with different nodes
      const node1Id = 'node-1';
      const node2Id = 'node-2';
      const node3Id = 'node-3';

      service1.initializeNodes(
        [createMockNode(node1Id, 'Node 1'), createMockNode(node2Id, 'Node 2')],
        { expanded: false }
      );

      service2.initializeNodes([createMockNode(node3Id, 'Node 3')], { expanded: false });

      // Register viewers
      NodeExpansionCoordinator.registerViewer(tab1Id, service1);
      NodeExpansionCoordinator.registerViewer(tab2Id, service2);

      // Expand different nodes in each tab
      service1.toggleExpanded(node1Id); // Tab 1: expand node-1
      service2.toggleExpanded(node3Id); // Tab 2: expand node-3

      // Extract expansion states
      const tab1Expanded = NodeExpansionCoordinator.getExpandedNodeIds(tab1Id);
      const tab2Expanded = NodeExpansionCoordinator.getExpandedNodeIds(tab2Id);

      // Verify independence
      expect(tab1Expanded).toContain(node1Id);
      expect(tab1Expanded).not.toContain(node3Id);
      expect(tab2Expanded).toContain(node3Id);
      expect(tab2Expanded).not.toContain(node1Id);
    });

    it('should gracefully handle missing nodes during restoration', () => {
      const tabId = 'test-tab-1';
      const service = createMockReactiveNodeService();

      const existingNodeId = 'existing-node';
      const deletedNodeId = 'deleted-node';

      service.initializeNodes([createMockNode(existingNodeId, 'Existing Node')], {
        expanded: false
      });

      NodeExpansionCoordinator.registerViewer(tabId, service);

      // Simulate restoring expansion state that includes a deleted node
      NodeExpansionCoordinator.restoreExpansionStates(tabId, [existingNodeId, deletedNodeId]);

      // Should only expand existing node, skip deleted node
      expect(service.getUIState(existingNodeId)?.expanded).toBe(true);
      // No error should be thrown for deleted node
    });

    it('should handle empty expansion state gracefully', () => {
      const tabId = 'test-tab-1';

      // Restore with empty array (simulates no expanded nodes)
      // This should handle gracefully without queueing
      NodeExpansionCoordinator.restoreExpansionStates(tabId, []);

      // Should not have queued anything
      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(0);

      // Register viewer with nodes after empty restoration
      const service = createMockReactiveNodeService();
      service.initializeNodes([createMockNode('node-1', 'Node 1')], { expanded: false });

      NodeExpansionCoordinator.registerViewer(tabId, service);

      // Should still be 0
      expect(NodeExpansionCoordinator.getPendingRestorationCount()).toBe(0);
    });
  });

  describe('TabPersistenceService Integration', () => {
    it('should include expansion states in persisted tab data', () => {
      const tabId = 'test-tab-1';
      const service = createMockReactiveNodeService();

      const node1Id = 'node-1';
      const node2Id = 'node-2';

      service.initializeNodes(
        [createMockNode(node1Id, 'Node 1'), createMockNode(node2Id, 'Node 2')],
        { expanded: false }
      );

      NodeExpansionCoordinator.registerViewer(tabId, service);

      // Expand node-1
      service.toggleExpanded(node1Id);

      // Extract expansion states
      const expandedNodeIds = NodeExpansionCoordinator.getExpandedNodeIds(tabId);

      // Create and save tab state
      const tabState: TabState = {
        tabs: [
          {
            id: tabId,
            title: 'Test Tab',
            type: 'node',
            content: { nodeId: node1Id },
            closeable: true,
            paneId: 'pane-1',
            expandedNodeIds: expandedNodeIds
          }
        ],
        panes: [{ id: 'pane-1', width: 100, tabIds: [tabId] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': tabId }
      };

      // Save immediately (bypass debounce for testing)
      TabPersistenceService.saveNow(tabState);

      // Load and verify
      const loaded = TabPersistenceService.load();
      expect(loaded?.tabs[0].expandedNodeIds).toEqual([node1Id]);
    });

    it('should handle tabs without expansion data', () => {
      const tabState: TabState = {
        tabs: [
          {
            id: 'tab-1',
            title: 'Test Tab',
            type: 'node',
            content: { nodeId: 'node-1' },
            closeable: true,
            paneId: 'pane-1'
            // No expandedNodeIds field
          }
        ],
        panes: [{ id: 'pane-1', width: 100, tabIds: ['tab-1'] }],
        activePaneId: 'pane-1',
        activeTabIds: { 'pane-1': 'tab-1' }
      };

      // Save immediately (bypass debounce for testing)
      TabPersistenceService.saveNow(tabState);

      const loaded = TabPersistenceService.load();
      expect(loaded?.tabs[0].expandedNodeIds).toBeUndefined();
    });
  });

  describe('Debouncing and Performance', () => {
    it('should handle rapid expansion changes efficiently', () => {
      const tabId = 'test-tab-1';
      const service = createMockReactiveNodeService();

      const nodeIds = Array.from({ length: 50 }, (_, i) => `node-${i}`);
      service.initializeNodes(
        nodeIds.map((id) => createMockNode(id, `Node ${id}`)),
        { expanded: false }
      );

      NodeExpansionCoordinator.registerViewer(tabId, service);

      // Rapidly toggle expansion on many nodes
      for (const nodeId of nodeIds) {
        service.toggleExpanded(nodeId);
      }

      // Extract expansion states
      const expandedNodeIds = NodeExpansionCoordinator.getExpandedNodeIds(tabId);

      // Should capture all expanded nodes
      expect(expandedNodeIds.length).toBe(50);
    });

    it('should use batch restoration for performance', () => {
      const tabId = 'test-tab-1';
      const service = createMockReactiveNodeService();

      // Create 20 nodes
      const nodeIds = Array.from({ length: 20 }, (_, i) => `node-${i}`);
      service.initializeNodes(
        nodeIds.map((id) => createMockNode(id, `Node ${id}`)),
        { expanded: false }
      );

      // Spy on batchSetExpanded to verify it's being used
      const batchSpy = vi.spyOn(service, 'batchSetExpanded');

      NodeExpansionCoordinator.registerViewer(tabId, service);

      // Restore expansion states for 15 nodes
      const nodesToExpand = nodeIds.slice(0, 15);
      NodeExpansionCoordinator.restoreExpansionStates(tabId, nodesToExpand);

      // Should have called batchSetExpanded once (not setExpanded 15 times)
      expect(batchSpy).toHaveBeenCalledOnce();
      expect(batchSpy).toHaveBeenCalledWith(
        nodesToExpand.map((nodeId) => ({ nodeId, expanded: true }))
      );

      // Verify all nodes are expanded
      for (const nodeId of nodesToExpand) {
        expect(service.getUIState(nodeId)?.expanded).toBe(true);
      }

      // Verify remaining nodes are still collapsed
      for (const nodeId of nodeIds.slice(15)) {
        expect(service.getUIState(nodeId)?.expanded).toBe(false);
      }
    });
  });
});

// Helper functions

function createMockReactiveNodeService() {
  return createReactiveNodeService({
    focusRequested: () => {},
    hierarchyChanged: () => {},
    nodeCreated: () => {},
    nodeDeleted: () => {}
  });
}

function createMockNode(id: string, content: string, _parentId: string | null = null): Node {
  return {
    id,
    nodeType: 'text',
    content,
    beforeSiblingId: null,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    properties: {},
    mentions: []
  };
}
