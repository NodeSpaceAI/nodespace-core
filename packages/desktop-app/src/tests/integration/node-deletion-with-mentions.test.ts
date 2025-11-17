/**
 * Node Deletion with Mentions Integration Test
 *
 * Tests that deleting nodes with @mention references doesn't cause
 * database locking errors. This verifies the fix for issue #190.
 *
 * Key behaviors tested:
 * 1. Node deletion with mentions completes without errors
 * 2. Cleanup doesn't race with database CASCADE deletes
 * 3. Cache invalidation happens correctly
 */
import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { NodeReferenceService } from '$lib/services/node-reference-service';
import { TauriNodeService } from '$lib/services/tauri-node-service';
import { HierarchyService } from '$lib/services/hierarchy-service';
import { eventBus } from '$lib/services/event-bus';
import type { Node } from '$lib/types';
import type { NodeDeletedEvent } from '$lib/services/event-types';
import type { ReactiveNodeService } from '$lib/services/reactive-node-service.svelte';

describe('Node Deletion with Mentions (Issue #190)', () => {
  let nodeReferenceService: NodeReferenceService;
  let mockTauriService: {
    queryNodes: ReturnType<typeof vi.fn>;
    createNode: ReturnType<typeof vi.fn>;
    getNode: ReturnType<typeof vi.fn>;
    updateNode: ReturnType<typeof vi.fn>;
    deleteNode: ReturnType<typeof vi.fn>;
    initializeDatabase: ReturnType<typeof vi.fn>;
    isInitialized: ReturnType<typeof vi.fn>;
  };
  let mockNodeManager: {
    findNode: ReturnType<typeof vi.fn>;
    nodes: Node[];
  };
  let mockHierarchyService: HierarchyService;

  beforeEach(() => {
    // Clear all event bus listeners (recreate eventBus if needed)
    // eventBus doesn't have a clear() method, but tests use isolated instances

    // Setup mock Tauri service
    mockTauriService = {
      queryNodes: vi.fn().mockResolvedValue([]),
      createNode: vi.fn().mockResolvedValue('new-node-id'),
      getNode: vi.fn().mockResolvedValue(null),
      updateNode: vi.fn().mockResolvedValue(undefined),
      deleteNode: vi.fn().mockResolvedValue(undefined),
      initializeDatabase: vi.fn().mockResolvedValue('/mock/path/db.sqlite'),
      isInitialized: vi.fn().mockReturnValue(true)
    };

    // Setup mock node manager
    mockNodeManager = {
      findNode: vi.fn().mockReturnValue(null),
      nodes: []
    };

    // Setup mock hierarchy service
    mockHierarchyService = {
      getNodePath: vi.fn().mockReturnValue({ nodeIds: [], depth: 0 })
    } as unknown as HierarchyService;

    // Create NodeReferenceService with mocks
    // Type casting: NodeReferenceService constructor expects full ReactiveNodeService interface,
    // but for this test we only need findNode() and nodes array.
    // Using 'unknown' requires explicit assertion which is safer than 'any'.
    nodeReferenceService = new NodeReferenceService(
      mockNodeManager as unknown as ReactiveNodeService,
      mockHierarchyService,
      mockTauriService as unknown as TauriNodeService
    );
  });

  afterEach(() => {
    // Cleanup after each test
  });

  describe('Deletion without Database Locking', () => {
    it('should not call queryNodes when node is deleted', async () => {
      // Create nodes with mention relationship
      const sourceNode: Node = {
        id: 'source-node',
        nodeType: 'text',
        content: 'Text with @target-node mention',
        beforeSiblingId: null,
        mentions: ['target-node'],
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      const targetNode: Node = {
        id: 'target-node',
        nodeType: 'text',
        content: 'Target node',
        beforeSiblingId: null,
        mentions: [],
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      // Setup mock responses
      mockNodeManager.findNode.mockImplementation((id: string) => {
        if (id === 'source-node') return sourceNode;
        if (id === 'target-node') return targetNode;
        return null;
      });

      // Emit node:deleted event (simulating delete operation)
      const deleteEvent: NodeDeletedEvent = {
        type: 'node:deleted',
        namespace: 'lifecycle',
        source: 'test',
        timestamp: Date.now(),
        nodeId: 'target-node',
      };

      eventBus.emit(deleteEvent);

      // Wait for any async operations
      await new Promise((resolve) => setTimeout(resolve, 100));

      // CRITICAL: queryNodes should NOT be called
      // The fix prevents the race condition by not querying during deletion
      expect(mockTauriService.queryNodes).not.toHaveBeenCalledWith({
        mentionedBy: 'target-node'
      });

      // updateNodeMentions is now private in NodeReferenceService
      // Database CASCADE handles cleanup automatically (no manual cleanup needed)
    });

    it('should invalidate caches when node is deleted', async () => {
      const nodeId = 'deleted-node';

      // Spy on private methods through the service
      // Using type assertion to access private method for testing purposes
      const clearCachesSpy = vi.spyOn(
        nodeReferenceService as NodeReferenceService & {
          invalidateNodeCaches: (nodeId: string) => void;
        },
        'invalidateNodeCaches'
      );

      // Emit deletion event
      const deleteEvent: NodeDeletedEvent = {
        type: 'node:deleted',
        namespace: 'lifecycle',
        source: 'test',
        timestamp: Date.now(),
        nodeId,
      };

      eventBus.emit(deleteEvent);

      // Wait for event processing
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Cache invalidation should have been called
      expect(clearCachesSpy).toHaveBeenCalledWith(nodeId);
    });

    it('should handle deletion of node with multiple mentions', async () => {
      // Create multiple nodes mentioning the target
      const targetNode: Node = {
        id: 'target',
        nodeType: 'text',
        content: 'Target',
        beforeSiblingId: null,
        mentions: [],
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      const mentioningNode1: Node = {
        id: 'mention-1',
        nodeType: 'text',
        content: 'Mentions @target',
        beforeSiblingId: null,
        mentions: ['target'],
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      const mentioningNode2: Node = {
        id: 'mention-2',
        nodeType: 'text',
        content: 'Also mentions @target',
        beforeSiblingId: null,
        mentions: ['target'],
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      mockNodeManager.findNode.mockImplementation((id: string) => {
        if (id === 'target') return targetNode;
        if (id === 'mention-1') return mentioningNode1;
        if (id === 'mention-2') return mentioningNode2;
        return null;
      });

      // Delete target node
      const deleteEvent: NodeDeletedEvent = {
        type: 'node:deleted',
        namespace: 'lifecycle',
        source: 'test',
        timestamp: Date.now(),
        nodeId: 'target',
      };

      eventBus.emit(deleteEvent);

      // Wait for processing
      await new Promise((resolve) => setTimeout(resolve, 100));

      // Should NOT query for nodes that mention the target
      expect(mockTauriService.queryNodes).not.toHaveBeenCalledWith({
        mentionedBy: 'target'
      });

      // Should NOT update mentions arrays
      // Database CASCADE handles this automatically
    });

    it('should not cause errors when deleting node without mentions', async () => {
      const simpleNode: Node = {
        id: 'simple-node',
        nodeType: 'text',
        content: 'Simple text without mentions',
        beforeSiblingId: null,
        mentions: [],
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        version: 1,
        properties: {}
      };

      mockNodeManager.findNode.mockReturnValue(simpleNode);

      // Delete simple node
      const deleteEvent: NodeDeletedEvent = {
        type: 'node:deleted',
        namespace: 'lifecycle',
        source: 'test',
        timestamp: Date.now(),
        nodeId: 'simple-node',
      };

      // Should not throw any errors
      expect(() => eventBus.emit(deleteEvent)).not.toThrow();

      await new Promise((resolve) => setTimeout(resolve, 50));

      // No database operations should have been triggered
      expect(mockTauriService.queryNodes).not.toHaveBeenCalled();
    });
  });

  describe('cleanupDeletedNodeReferences deprecation', () => {
    it('cleanupDeletedNodeReferences should not be called automatically', async () => {
      // Spy on the deprecated method
      // Using type assertion to access private method for testing purposes
      const cleanupSpy = vi.spyOn(
        nodeReferenceService as NodeReferenceService & {
          cleanupDeletedNodeReferences: () => Promise<void>;
        },
        'cleanupDeletedNodeReferences'
      );

      // Emit deletion event
      const deleteEvent: NodeDeletedEvent = {
        type: 'node:deleted',
        namespace: 'lifecycle',
        source: 'test',
        timestamp: Date.now(),
        nodeId: 'some-node',
      };

      eventBus.emit(deleteEvent);

      await new Promise((resolve) => setTimeout(resolve, 100));

      // The deprecated cleanup function should NOT be called
      expect(cleanupSpy).not.toHaveBeenCalled();
    });
  });
});
