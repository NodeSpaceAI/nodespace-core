import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { browserSyncService } from '$lib/services/browser-sync-service';
import { SharedNodeStore, sharedNodeStore } from '$lib/services/shared-node-store';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import type { SseEvent } from '$lib/types/sse-events';
import type { Node } from '$lib/types';

/**
 * Tests for BrowserSyncService SSE Event Ordering
 *
 * Verifies that BrowserSyncService correctly handles SSE events that arrive
 * out-of-order from the server, preventing race conditions in ReactiveStructureTree
 * and SharedNodeStore.
 *
 * ## Event Ordering Guarantees
 *
 * SSE events from the dev-proxy may arrive out-of-order due to:
 * - Network latency variations
 * - Buffering in the SSE stream
 * - Timing differences between event creation and transmission
 *
 * Currently, BrowserSyncService applies events immediately without buffering
 * or ordering validation. This means:
 * - No automatic deduplication or replay detection
 * - No guaranteed order between related events
 * - Race conditions possible if events arrive in wrong order
 *
 * However, ReactiveStructureTree and SharedNodeStore have defensive measures:
 * - addChild() handles duplicate edges gracefully
 * - removeChild() handles missing edges gracefully
 * - setNode() overwrites with latest data
 *
 * ## Testing Strategy
 *
 * Tests verify that the system handles out-of-order events without crashes
 * or data loss, even when events arrive in wrong order.
 */

/**
 * Helper to create test nodes with proper schema
 */
function createTestNode(id: string, content = 'Test node'): Node {
  return {
    id,
    nodeType: 'text',
    content,
    properties: {},
    mentions: [],
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1
  };
}

describe('BrowserSyncService - SSE Event Ordering', () => {
  beforeEach(() => {
    // Reset and clear stores before each test
    SharedNodeStore.resetInstance();
    structureTree.children.clear();
  });

  afterEach(() => {
    // Cleanup
    sharedNodeStore.clearAll();
    structureTree.children.clear();
    SharedNodeStore.resetInstance();
  });

  describe('Event Ordering Guarantees', () => {
    it('should document that events are processed in arrival order (not guaranteed to be correct order)', () => {
      // This test documents the current behavior:
      // BrowserSyncService processes events in the order they arrive from SSE,
      // which may not be the order they occurred on the server.

      // Example: Backend operations:
      //   1. Create node N1
      //   2. Create edge P->N1
      // SSE received (wrong order):
      //   1. Edge created (P->N1)
      //   2. Node created (N1)

      // Currently, the edge would be added to tree before node exists in store.
      // This is handled gracefully by the stores, but is worth documenting.

      expect(browserSyncService).toBeDefined();
    });

    it('should handle network latency causing events to arrive out of order', () => {
      // Simulate: Create node, then create edge (correct order)
      const nodeData = createTestNode('node1');

      // But events arrive in reverse order (edge first, then node)
      // First, the edge event arrives
      const edgeEvent: SseEvent = {
        type: 'edgeCreated',
        parentId: 'parent1',
        childId: 'node1'
      };

      // Then, the node event arrives
      const nodeEvent: SseEvent = {
        type: 'nodeCreated',
        nodeId: 'node1',
        nodeData
      };

      // Call handleEvent in reverse order (simulating network latency)
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent(edgeEvent);

      // At this point, edge references a node that hasn't been created yet
      // This should not crash
      expect(structureTree.hasChildren('parent1')).toBe(true);
      expect(structureTree.getChildren('parent1')).toContain('node1');

      // Now the node arrives
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent(nodeEvent);

      // Both should now be in sync
      expect(sharedNodeStore.getNode('node1')).toBeDefined();
      expect(structureTree.getChildren('parent1')).toContain('node1');
    });
  });

  describe('Edge before Node Race Condition', () => {
    it('should handle edge created before referenced node exists', () => {
      // Scenario: Backend creates node N1 with parent P1, but SSE delivers
      // edge:created event before nodeCreated event

      const edgeEvent: SseEvent = {
        type: 'edgeCreated',
        parentId: 'parent1',
        childId: 'child1'
      };

      // Add edge first (node doesn't exist yet)
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent(edgeEvent);

      // Verify edge was added even though node doesn't exist
      expect(structureTree.hasChildren('parent1')).toBe(true);
      expect(structureTree.getChildren('parent1')).toContain('child1');

      // Node doesn't exist in store yet
      expect(sharedNodeStore.getNode('child1')).toBeUndefined();

      // Now create the node
      const nodeData = createTestNode('child1', 'Created after edge');

      const nodeEvent: SseEvent = {
        type: 'nodeCreated',
        nodeId: 'child1',
        nodeData
      };

      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent(nodeEvent);

      // Now everything is in sync
      expect(sharedNodeStore.getNode('child1')).toBeDefined();
      expect(structureTree.getChildren('parent1')).toContain('child1');
    });

    it('should handle duplicate edge to same parent (second edge rejected due to tree invariant)', () => {
      // Scenario: Two edges to the same parent (duplicate) before node:created
      // Note: ReactiveStructureTree enforces a tree structure (not DAG),
      // so a node can only have one parent. Attempting to add a second parent
      // is logged as a tree invariant violation and rejected.

      const edge1: SseEvent = {
        type: 'edgeCreated',
        parentId: 'parent1',
        childId: 'child'
      };

      const edge2: SseEvent = {
        type: 'edgeCreated',
        parentId: 'parent2',
        childId: 'child'
      };

      // First edge arrives
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent(edge1);

      // Second edge to different parent arrives (violates tree structure)
      // This is logged as an error but doesn't crash
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent(edge2);

      // First parent relationship exists
      expect(structureTree.getChildren('parent1')).toContain('child');
      // Second parent relationship is rejected (tree invariant violation)
      expect(structureTree.getChildren('parent2')).not.toContain('child');

      // Now create the node
      const nodeData = createTestNode('child');

      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent({
        type: 'nodeCreated',
        nodeId: 'child',
        nodeData
      });

      // Verify state is consistent with tree structure
      expect(sharedNodeStore.getNode('child')).toBeDefined();
      expect(structureTree.getChildren('parent1')).toContain('child');
      expect(structureTree.getChildren('parent2')).not.toContain('child');
    });
  });

  describe('Node before Edge Deletion Race Condition', () => {
    it('should handle node deleted before edge deleted', () => {
      // Scenario: Node is deleted, then its edges should be deleted, but
      // edge:deleted event might arrive before or after node:deleted

      // First, set up node and edge
      const nodeData = createTestNode('child1', 'Node to delete');

      sharedNodeStore.setNode(nodeData, { type: 'database', reason: 'sse-sync' });
      structureTree.addChild({ parentId: 'parent1', childId: 'child1', order: 1 });

      // Verify initial state
      expect(sharedNodeStore.getNode('child1')).toBeDefined();
      expect(structureTree.getChildren('parent1')).toContain('child1');

      // Node is deleted first
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent({
        type: 'nodeDeleted',
        nodeId: 'child1'
      });

      // Node should be gone
      expect(sharedNodeStore.getNode('child1')).toBeUndefined();
      // But edge might still reference it (until edge:deleted arrives)
      expect(structureTree.getChildren('parent1')).toContain('child1');

      // Now the edge is deleted (arriving late)
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent({
        type: 'edgeDeleted',
        parentId: 'parent1',
        childId: 'child1'
      });

      // Now everything is cleaned up
      expect(sharedNodeStore.getNode('child1')).toBeUndefined();
      expect(structureTree.getChildren('parent1')).not.toContain('child1');
    });

    it('should handle edge deleted before node deleted', () => {
      // Opposite scenario: edge is deleted before node is deleted

      const nodeData = createTestNode('child1', 'Node to delete');

      // Initial setup
      sharedNodeStore.setNode(nodeData, { type: 'database', reason: 'sse-sync' });
      structureTree.addChild({ parentId: 'parent1', childId: 'child1', order: 1 });

      // Edge is deleted first
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent({
        type: 'edgeDeleted',
        parentId: 'parent1',
        childId: 'child1'
      });

      // Edge should be gone
      expect(structureTree.getChildren('parent1')).not.toContain('child1');
      // But node still exists (until node:deleted arrives)
      expect(sharedNodeStore.getNode('child1')).toBeDefined();

      // Node is deleted (arriving late)
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent({
        type: 'nodeDeleted',
        nodeId: 'child1'
      });

      // Now everything is cleaned up
      expect(sharedNodeStore.getNode('child1')).toBeUndefined();
      expect(structureTree.getChildren('parent1')).not.toContain('child1');
    });
  });

  describe('Bulk Operations with Interleaved Events', () => {
    it('should handle bulk create of multiple nodes with edges arriving interleaved', () => {
      // Scenario: Creating a tree:
      //   P
      //  / \
      // N1 N2
      //    |
      //    N3
      //
      // But events arrive interleaved:
      // - edge P->N1, edge P->N2, edge N2->N3, node N1, node N2, node N3

      // Edges arrive first
      const edges: SseEvent[] = [
        { type: 'edgeCreated', parentId: 'P', childId: 'N1' },
        { type: 'edgeCreated', parentId: 'P', childId: 'N2' },
        { type: 'edgeCreated', parentId: 'N2', childId: 'N3' }
      ];

      for (const edge of edges) {
        // @ts-expect-error - accessing private method for testing
        browserSyncService.handleEvent(edge);
      }

      // All edges should be in place
      expect(structureTree.getChildren('P').sort()).toEqual(['N1', 'N2']);
      expect(structureTree.getChildren('N2')).toEqual(['N3']);

      // Now nodes arrive in random order
      const nodes: SseEvent[] = [
        {
          type: 'nodeCreated',
          nodeId: 'N2',
          nodeData: createTestNode('N2', 'Node 2')
        },
        {
          type: 'nodeCreated',
          nodeId: 'N1',
          nodeData: createTestNode('N1', 'Node 1')
        },
        {
          type: 'nodeCreated',
          nodeId: 'N3',
          nodeData: createTestNode('N3', 'Node 3')
        }
      ];

      for (const node of nodes) {
        // @ts-expect-error - accessing private method for testing
        browserSyncService.handleEvent(node);
      }

      // Everything should be in place
      expect(sharedNodeStore.getNode('N1')).toBeDefined();
      expect(sharedNodeStore.getNode('N2')).toBeDefined();
      expect(sharedNodeStore.getNode('N3')).toBeDefined();
      expect(structureTree.getChildren('P').sort()).toEqual(['N1', 'N2']);
      expect(structureTree.getChildren('N2')).toEqual(['N3']);
    });

    it('should handle duplicate edge events (idempotent)', () => {
      // Scenario: Edge creation event gets sent twice (retransmission)

      const nodeData = createTestNode('child1');

      // Set up node
      sharedNodeStore.setNode(nodeData, { type: 'database', reason: 'sse-sync' });

      // Create edge first time
      const edgeEvent: SseEvent = {
        type: 'edgeCreated',
        parentId: 'parent1',
        childId: 'child1'
      };

      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent(edgeEvent);

      expect(structureTree.getChildren('parent1')).toEqual(['child1']);

      // Duplicate edge event arrives (idempotent operation)
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent(edgeEvent);

      // Should still have one edge, not duplicated
      expect(structureTree.getChildren('parent1')).toEqual(['child1']);
    });

    it('should handle node update events preserving structure', () => {
      // Scenario: Node is updated while structure events arrive

      const initialNode = createTestNode('node1', 'Original content');

      // Create node and edge
      sharedNodeStore.setNode(initialNode, { type: 'database', reason: 'sse-sync' });
      structureTree.addChild({ parentId: 'parent1', childId: 'node1', order: 1 });

      // Update event arrives
      const updatedNode: Node = {
        ...initialNode,
        content: 'Updated content',
        modifiedAt: new Date().toISOString(),
        version: 2
      };

      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent({
        type: 'nodeUpdated',
        nodeId: 'node1',
        nodeData: updatedNode
      });

      // Content should be updated
      const retrieved = sharedNodeStore.getNode('node1');
      expect(retrieved?.content).toBe('Updated content');

      // Structure should be preserved
      expect(structureTree.getChildren('parent1')).toContain('node1');
    });
  });

  describe('Concurrent Operations', () => {
    it('should handle concurrent modifications to multiple node trees', () => {
      // Scenario: Two independent trees are modified simultaneously:
      // Tree 1: P1 -> N1 -> N2
      // Tree 2: P2 -> N3 -> N4

      // Interleaved events
      const events: SseEvent[] = [
        // Tree 1 setup
        {
          type: 'nodeCreated',
          nodeId: 'P1',
          nodeData: createTestNode('P1', 'Parent 1')
        },
        // Tree 2 setup
        {
          type: 'nodeCreated',
          nodeId: 'P2',
          nodeData: createTestNode('P2', 'Parent 2')
        },
        // Tree 1 edges and nodes
        { type: 'edgeCreated', parentId: 'P1', childId: 'N1' },
        {
          type: 'nodeCreated',
          nodeId: 'N1',
          nodeData: createTestNode('N1', 'Node 1')
        },
        // Tree 2 edges and nodes
        { type: 'edgeCreated', parentId: 'P2', childId: 'N3' },
        {
          type: 'nodeCreated',
          nodeId: 'N3',
          nodeData: createTestNode('N3', 'Node 3')
        }
      ];

      for (const event of events) {
        // @ts-expect-error - accessing private method for testing
        browserSyncService.handleEvent(event);
      }

      // Both trees should exist independently
      expect(structureTree.getChildren('P1')).toContain('N1');
      expect(structureTree.getChildren('P2')).toContain('N3');
      expect(sharedNodeStore.getNode('N1')).toBeDefined();
      expect(sharedNodeStore.getNode('N3')).toBeDefined();
    });
  });

  describe('Edge Cases and Error Handling', () => {
    it('should handle edge events for non-existent nodes gracefully', () => {
      // This can happen if the node:deleted event arrived before edge:deleted

      const edgeEvent: SseEvent = {
        type: 'edgeDeleted',
        parentId: 'non-existent-parent',
        childId: 'non-existent-child'
      };

      // Should not crash
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent(edgeEvent);

      // No errors, state unchanged
      expect(structureTree.getChildren('non-existent-parent')).toEqual([]);
    });

    it('should handle delete of already-deleted node gracefully', () => {
      // Node is deleted twice (e.g., retry logic on backend)

      const nodeData = createTestNode('node1', 'Node to delete');

      // Create node
      sharedNodeStore.setNode(nodeData, { type: 'database', reason: 'sse-sync' });

      // Delete it
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent({
        type: 'nodeDeleted',
        nodeId: 'node1'
      });

      expect(sharedNodeStore.getNode('node1')).toBeUndefined();

      // Delete it again (should not crash)
      // @ts-expect-error - accessing private method for testing
      browserSyncService.handleEvent({
        type: 'nodeDeleted',
        nodeId: 'node1'
      });

      expect(sharedNodeStore.getNode('node1')).toBeUndefined();
    });
  });
});
