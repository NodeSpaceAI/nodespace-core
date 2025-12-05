import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { browserSyncService } from '$lib/services/browser-sync-service';
import { SharedNodeStore, sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { getClientId } from '$lib/services/client-id';
import type { SseEvent } from '$lib/types/sse-events';
import type { Node } from '$lib/types';
import * as backendAdapterModule from '$lib/services/backend-adapter';

/**
 * Type-safe test interface for accessing private methods.
 * This explicit type cast is the approved pattern for testing private methods
 * without requiring @ts-expect-error annotations throughout tests.
 */
interface TestableBrowserSyncService {
  handleEvent(event: SseEvent): void;
}

const testableService = browserSyncService as unknown as TestableBrowserSyncService;

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
 * Issue #724: Events now send only nodeId (not full payload).
 * Tests mock backendAdapter.getNode to return test data.
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

/**
 * Mock getNode to return test data based on nodeId
 */
const mockNodes = new Map<string, Node>();

function setupMockGetNode() {
  vi.spyOn(backendAdapterModule.backendAdapter, 'getNode').mockImplementation(async (id: string) => {
    return mockNodes.get(id) || null;
  });
}

/**
 * Register a node to be returned by mocked getNode
 */
function registerMockNode(node: Node) {
  mockNodes.set(node.id, node);
}

describe('BrowserSyncService - SSE Event Ordering', () => {
  beforeEach(() => {
    // Reset and clear stores before each test
    SharedNodeStore.resetInstance();
    structureTree.children.clear();
    mockNodes.clear();
    setupMockGetNode();
  });

  afterEach(() => {
    // Cleanup
    sharedNodeStore.clearAll();
    structureTree.children.clear();
    SharedNodeStore.resetInstance();
    mockNodes.clear();
    vi.restoreAllMocks();
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

    it('should handle network latency causing events to arrive out of order', async () => {
      // Simulate: Create node, then create edge (correct order)
      const nodeData = createTestNode('node1');
      registerMockNode(nodeData);

      // But events arrive in reverse order (edge first, then node)
      // First, the edge event arrives
      const edgeEvent: SseEvent = {
        type: 'edgeCreated',
        parentId: 'parent1',
        childId: 'node1'
      };

      // Then, the node event arrives (Issue #724: ID-only)
      const nodeEvent: SseEvent = {
        type: 'nodeCreated',
        nodeId: 'node1'
      };

      // Call handleEvent in reverse order (simulating network latency)
      testableService.handleEvent(edgeEvent);

      // At this point, edge references a node that hasn't been created yet
      // This should not crash
      expect(structureTree.hasChildren('parent1')).toBe(true);
      expect(structureTree.getChildren('parent1')).toContain('node1');

      // Now the node arrives (triggers async fetch)
      testableService.handleEvent(nodeEvent);

      // Wait for async fetch to complete
      await vi.waitFor(() => {
        expect(sharedNodeStore.getNode('node1')).toBeDefined();
      });

      // Both should now be in sync
      expect(structureTree.getChildren('parent1')).toContain('node1');
    });
  });

  describe('Edge before Node Race Condition', () => {
    it('should handle edge created before referenced node exists', async () => {
      // Scenario: Backend creates node N1 with parent P1, but SSE delivers
      // edge:created event before nodeCreated event

      const nodeData = createTestNode('child1', 'Created after edge');
      registerMockNode(nodeData);

      const edgeEvent: SseEvent = {
        type: 'edgeCreated',
        parentId: 'parent1',
        childId: 'child1'
      };

      // Add edge first (node doesn't exist yet)
      testableService.handleEvent(edgeEvent);

      // Verify edge was added even though node doesn't exist
      expect(structureTree.hasChildren('parent1')).toBe(true);
      expect(structureTree.getChildren('parent1')).toContain('child1');

      // Node doesn't exist in store yet
      expect(sharedNodeStore.getNode('child1')).toBeUndefined();

      // Now create the node event (Issue #724: ID-only)
      const nodeEvent: SseEvent = {
        type: 'nodeCreated',
        nodeId: 'child1'
      };

      testableService.handleEvent(nodeEvent);

      // Wait for async fetch to complete
      await vi.waitFor(() => {
        expect(sharedNodeStore.getNode('child1')).toBeDefined();
      });

      // Now everything is in sync
      expect(structureTree.getChildren('parent1')).toContain('child1');
    });

    it('should handle duplicate edge to same parent (second edge rejected due to tree invariant)', async () => {
      // Scenario: Two edges to the same parent (duplicate) before node:created
      // Note: ReactiveStructureTree enforces a tree structure (not DAG),
      // so a node can only have one parent. Attempting to add a second parent
      // is logged as a tree invariant violation and rejected.

      const nodeData = createTestNode('child');
      registerMockNode(nodeData);

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
      testableService.handleEvent(edge1);

      // Second edge to different parent arrives (violates tree structure)
      // This is logged as an error but doesn't crash
      testableService.handleEvent(edge2);

      // First parent relationship exists
      expect(structureTree.getChildren('parent1')).toContain('child');
      // Second parent relationship is rejected (tree invariant violation)
      expect(structureTree.getChildren('parent2')).not.toContain('child');

      // Now create the node (Issue #724: ID-only)
      testableService.handleEvent({
        type: 'nodeCreated',
        nodeId: 'child'
      });

      // Wait for async fetch
      await vi.waitFor(() => {
        expect(sharedNodeStore.getNode('child')).toBeDefined();
      });

      // Verify state is consistent with tree structure
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
      testableService.handleEvent({
        type: 'nodeDeleted',
        nodeId: 'child1'
      });

      // Node should be gone
      expect(sharedNodeStore.getNode('child1')).toBeUndefined();
      // But edge might still reference it (until edge:deleted arrives)
      expect(structureTree.getChildren('parent1')).toContain('child1');

      // Now the edge is deleted (arriving late)
      testableService.handleEvent({
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
      testableService.handleEvent({
        type: 'edgeDeleted',
        parentId: 'parent1',
        childId: 'child1'
      });

      // Edge should be gone
      expect(structureTree.getChildren('parent1')).not.toContain('child1');
      // But node still exists (until node:deleted arrives)
      expect(sharedNodeStore.getNode('child1')).toBeDefined();

      // Node is deleted (arriving late)
      testableService.handleEvent({
        type: 'nodeDeleted',
        nodeId: 'child1'
      });

      // Now everything is cleaned up
      expect(sharedNodeStore.getNode('child1')).toBeUndefined();
      expect(structureTree.getChildren('parent1')).not.toContain('child1');
    });
  });

  describe('Bulk Operations with Interleaved Events', () => {
    it('should handle bulk create of multiple nodes with edges arriving interleaved', async () => {
      // Scenario: Creating a tree:
      //   P
      //  / \
      // N1 N2
      //    |
      //    N3
      //
      // But events arrive interleaved:
      // - edge P->N1, edge P->N2, edge N2->N3, node N1, node N2, node N3

      // Register mock nodes
      registerMockNode(createTestNode('N1', 'Node 1'));
      registerMockNode(createTestNode('N2', 'Node 2'));
      registerMockNode(createTestNode('N3', 'Node 3'));

      // Edges arrive first
      const edges: SseEvent[] = [
        { type: 'edgeCreated', parentId: 'P', childId: 'N1' },
        { type: 'edgeCreated', parentId: 'P', childId: 'N2' },
        { type: 'edgeCreated', parentId: 'N2', childId: 'N3' }
      ];

      for (const edge of edges) {
        testableService.handleEvent(edge);
      }

      // All edges should be in place
      expect(structureTree.getChildren('P').sort()).toEqual(['N1', 'N2']);
      expect(structureTree.getChildren('N2')).toEqual(['N3']);

      // Now nodes arrive in random order (Issue #724: ID-only)
      const nodes: SseEvent[] = [
        { type: 'nodeCreated', nodeId: 'N2' },
        { type: 'nodeCreated', nodeId: 'N1' },
        { type: 'nodeCreated', nodeId: 'N3' }
      ];

      for (const node of nodes) {
        testableService.handleEvent(node);
      }

      // Wait for all async fetches to complete
      await vi.waitFor(() => {
        expect(sharedNodeStore.getNode('N1')).toBeDefined();
        expect(sharedNodeStore.getNode('N2')).toBeDefined();
        expect(sharedNodeStore.getNode('N3')).toBeDefined();
      });

      // Everything should be in place
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

      testableService.handleEvent(edgeEvent);

      expect(structureTree.getChildren('parent1')).toEqual(['child1']);

      // Duplicate edge event arrives (idempotent operation)
      testableService.handleEvent(edgeEvent);

      // Should still have one edge, not duplicated
      expect(structureTree.getChildren('parent1')).toEqual(['child1']);
    });

    it('should handle node update events preserving structure', async () => {
      // Scenario: Node is updated while structure events arrive

      const initialNode = createTestNode('node1', 'Original content');

      // Create node and edge
      sharedNodeStore.setNode(initialNode, { type: 'database', reason: 'sse-sync' });
      structureTree.addChild({ parentId: 'parent1', childId: 'node1', order: 1 });

      // Update mock data
      const updatedNode: Node = {
        ...initialNode,
        content: 'Updated content',
        modifiedAt: new Date().toISOString(),
        version: 2
      };
      registerMockNode(updatedNode);

      // Update event arrives (Issue #724: ID-only)
      testableService.handleEvent({
        type: 'nodeUpdated',
        nodeId: 'node1'
      });

      // Wait for async fetch
      await vi.waitFor(() => {
        const retrieved = sharedNodeStore.getNode('node1');
        expect(retrieved?.content).toBe('Updated content');
      });

      // Structure should be preserved
      expect(structureTree.getChildren('parent1')).toContain('node1');
    });
  });

  describe('Concurrent Operations', () => {
    it('should handle concurrent modifications to multiple node trees', async () => {
      // Scenario: Two independent trees are modified simultaneously:
      // Tree 1: P1 -> N1 -> N2
      // Tree 2: P2 -> N3 -> N4

      // Register mock nodes
      registerMockNode(createTestNode('P1', 'Parent 1'));
      registerMockNode(createTestNode('P2', 'Parent 2'));
      registerMockNode(createTestNode('N1', 'Node 1'));
      registerMockNode(createTestNode('N3', 'Node 3'));

      // Interleaved events (Issue #724: ID-only for node events)
      const events: SseEvent[] = [
        // Tree 1 setup
        { type: 'nodeCreated', nodeId: 'P1' },
        // Tree 2 setup
        { type: 'nodeCreated', nodeId: 'P2' },
        // Tree 1 edges and nodes
        { type: 'edgeCreated', parentId: 'P1', childId: 'N1' },
        { type: 'nodeCreated', nodeId: 'N1' },
        // Tree 2 edges and nodes
        { type: 'edgeCreated', parentId: 'P2', childId: 'N3' },
        { type: 'nodeCreated', nodeId: 'N3' }
      ];

      for (const event of events) {
        testableService.handleEvent(event);
      }

      // Wait for all async fetches to complete
      await vi.waitFor(() => {
        expect(sharedNodeStore.getNode('N1')).toBeDefined();
        expect(sharedNodeStore.getNode('N3')).toBeDefined();
      });

      // Both trees should exist independently
      expect(structureTree.getChildren('P1')).toContain('N1');
      expect(structureTree.getChildren('P2')).toContain('N3');
    });
  });

  describe('ClientId Filtering', () => {
    /**
     * SSE ClientId Filtering Architecture
     *
     * The clientId filtering is implemented SERVER-SIDE in dev-proxy.rs:
     * 1. Browser connects to SSE endpoint with ?clientId=xxx query param
     * 2. Server stores clientId per SSE connection
     * 3. When backend emits events, server checks event.clientId vs connection.clientId
     * 4. Events from same clientId are NOT sent to that client (prevents echo)
     *
     * This means:
     * - BrowserSyncService is responsible for SENDING clientId on connect
     * - Server is responsible for FILTERING events
     * - Client-side tests verify correct clientId handling in received events
     */

    it('should pass clientId to SSE endpoint via getClientId()', () => {
      // Verify that getClientId returns a value (imported at top of file)
      // The actual filtering happens server-side, but the client must provide the clientId
      const clientId = getClientId();

      // Should return a valid UUID (from sessionStorage or newly generated)
      // When window.sessionStorage is available (happy-dom), returns UUID
      // When window is undefined (SSR), returns 'test-client'
      expect(clientId).toBeTruthy();
      expect(typeof clientId).toBe('string');
      expect(clientId.length).toBeGreaterThan(0);
    });

    it('should process events with different clientId (from other clients)', async () => {
      // When an event arrives with a different clientId (or no clientId),
      // it means the server determined this client should receive it.
      // The client should process it normally.

      const nodeData = createTestNode('other-client-node', 'Created by another client');
      registerMockNode(nodeData);

      // Issue #724: ID-only event
      const event: SseEvent = {
        type: 'nodeCreated',
        nodeId: 'other-client-node',
        clientId: 'different-client-123' // Different from our test-client ID
      };

      testableService.handleEvent(event);

      // Wait for async fetch
      await vi.waitFor(() => {
        expect(sharedNodeStore.getNode('other-client-node')).toBeDefined();
      });

      // Event should be processed - node should be in store
      expect(sharedNodeStore.getNode('other-client-node')?.content).toBe(
        'Created by another client'
      );
    });

    it('should process events with no clientId (backward compatibility)', async () => {
      // Events without clientId should still be processed
      // This ensures backward compatibility with older event sources

      const nodeData = createTestNode('legacy-node', 'Legacy event without clientId');
      registerMockNode(nodeData);

      // Issue #724: ID-only event, no clientId
      const event: SseEvent = {
        type: 'nodeCreated',
        nodeId: 'legacy-node'
        // Note: no clientId field
      };

      testableService.handleEvent(event);

      // Wait for async fetch
      await vi.waitFor(() => {
        expect(sharedNodeStore.getNode('legacy-node')).toBeDefined();
      });
    });

    it('should document that events with matching clientId are filtered server-side', () => {
      /**
       * IMPORTANT: This test documents the architecture, not client behavior.
       *
       * The dev-proxy (server) filters events BEFORE sending them to clients:
       * - Client A connects with clientId="A"
       * - Client A creates a node (sends X-Client-Id header)
       * - Server stores clientId="A" with the event
       * - When broadcasting, server skips Client A for events with clientId="A"
       *
       * This means:
       * - BrowserSyncService NEVER receives events it originated
       * - No client-side filtering is needed
       * - This test verifies the architecture is documented
       *
       * See: packages/desktop-app/src-tauri/src/bin/dev-proxy.rs
       * Function: sse_handler() - lines 516-575
       */

      // If the server-side filtering fails, the client would see its own events.
      // This is handled gracefully by the stores (duplicate detection, version checks)
      // but should not happen in normal operation.

      expect(browserSyncService).toBeDefined();
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
      testableService.handleEvent(edgeEvent);

      // No errors, state unchanged
      expect(structureTree.getChildren('non-existent-parent')).toEqual([]);
    });

    it('should handle delete of already-deleted node gracefully', () => {
      // Node is deleted twice (e.g., retry logic on backend)

      const nodeData = createTestNode('node1', 'Node to delete');

      // Create node
      sharedNodeStore.setNode(nodeData, { type: 'database', reason: 'sse-sync' });

      // Delete it
      testableService.handleEvent({
        type: 'nodeDeleted',
        nodeId: 'node1'
      });

      expect(sharedNodeStore.getNode('node1')).toBeUndefined();

      // Delete it again (should not crash)
      testableService.handleEvent({
        type: 'nodeDeleted',
        nodeId: 'node1'
      });

      expect(sharedNodeStore.getNode('node1')).toBeUndefined();
    });
  });
});
