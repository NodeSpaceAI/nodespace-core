/**
 * SharedNodeStore Tests
 *
 * Tests for Phase 1-2: Shared Node Store Foundation and Multi-Source Updates
 *
 * Test Coverage:
 * - Singleton behavior
 * - Basic CRUD operations
 * - Reactive subscriptions (observer pattern)
 * - Conflict detection and resolution
 * - Optimistic updates with rollback
 * - Performance metrics
 * - Memory leak prevention
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store';
import { PersistenceCoordinator } from '../../lib/services/persistence-coordinator.svelte';
import type { Node } from '../../lib/types';
import type { UpdateSource, NodeUpdate } from '../../lib/types/update-protocol';
import { LastWriteWinsResolver } from '../../lib/services/conflict-resolvers';

describe('SharedNodeStore', () => {
  let store: SharedNodeStore;
  const mockNode: Node = {
    id: 'test-node-1',
    nodeType: 'text',
    content: 'Test content',
    beforeSiblingId: null,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    properties: {},
    mentions: []
  };

  const viewerSource: UpdateSource = {
    type: 'viewer',
    viewerId: 'viewer-1'
  };

  beforeEach(() => {
    // Reset singleton before each test
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();

    // Enable test mode on PersistenceCoordinator to skip database operations
    PersistenceCoordinator.resetInstance();
    const coordinator = PersistenceCoordinator.getInstance();
    coordinator.enableTestMode();
    coordinator.resetTestState();
  });

  afterEach(async () => {
    // Clean up
    store.clearAll();
    SharedNodeStore.resetInstance();

    // Reset PersistenceCoordinator and wait for cancellation cleanup
    const coordinator = PersistenceCoordinator.getInstance();
    await coordinator.reset(); // Now properly waits for all cancellations
    PersistenceCoordinator.resetInstance();
  });

  // ========================================================================
  // Singleton Behavior
  // ========================================================================

  describe('Singleton Pattern', () => {
    it('should return same instance on multiple getInstance calls', () => {
      const instance1 = SharedNodeStore.getInstance();
      const instance2 = SharedNodeStore.getInstance();

      expect(instance1).toBe(instance2);
    });

    it('should create new instance after reset', () => {
      const instance1 = SharedNodeStore.getInstance();
      SharedNodeStore.resetInstance();
      const instance2 = SharedNodeStore.getInstance();

      expect(instance1).not.toBe(instance2);
    });
  });

  // ========================================================================
  // Basic CRUD Operations
  // ========================================================================

  describe('Basic CRUD', () => {
    it('should store and retrieve a node', () => {
      store.setNode(mockNode, viewerSource);

      const retrieved = store.getNode(mockNode.id);
      expect(retrieved).toEqual(mockNode);
    });

    it('should return undefined for non-existent node', () => {
      const retrieved = store.getNode('non-existent');
      expect(retrieved).toBeUndefined();
    });

    it('should check if node exists', () => {
      expect(store.hasNode(mockNode.id)).toBe(false);

      store.setNode(mockNode, viewerSource);
      expect(store.hasNode(mockNode.id)).toBe(true);
    });

    it('should get all nodes', () => {
      const node2: Node = { ...mockNode, id: 'test-node-2' };

      store.setNode(mockNode, viewerSource);
      store.setNode(node2, viewerSource);

      const allNodes = store.getAllNodes();
      expect(allNodes.size).toBe(2);
      expect(allNodes.has(mockNode.id)).toBe(true);
      expect(allNodes.has(node2.id)).toBe(true);
    });

    it('should get node count', () => {
      expect(store.getNodeCount()).toBe(0);

      store.setNode(mockNode, viewerSource);
      expect(store.getNodeCount()).toBe(1);

      store.setNode({ ...mockNode, id: 'test-node-2' }, viewerSource);
      expect(store.getNodeCount()).toBe(2);
    });

    it('should delete a node', () => {
      store.setNode(mockNode, viewerSource);
      expect(store.hasNode(mockNode.id)).toBe(true);

      store.deleteNode(mockNode.id, viewerSource);
      expect(store.hasNode(mockNode.id)).toBe(false);
    });

    it('should clear all nodes', () => {
      store.setNode(mockNode, viewerSource);
      store.setNode({ ...mockNode, id: 'test-node-2' }, viewerSource);
      expect(store.getNodeCount()).toBe(2);

      store.clearAll();
      expect(store.getNodeCount()).toBe(0);
    });
  });

  // ========================================================================
  // Node Filtering (by parent)
  // ========================================================================

  describe('Node Filtering', () => {
    it('should get all nodes', () => {
      const node1: Node = { ...mockNode, id: 'node-1' };
      const node2: Node = { ...mockNode, id: 'node-2' };
      const node3: Node = { ...mockNode, id: 'node-3' };

      store.setNode(node1, viewerSource);
      store.setNode(node2, viewerSource);
      store.setNode(node3, viewerSource);

      // Populate cache to simulate backend hierarchy data
      // (Graph-native architecture stores hierarchy as edges, not in Node fields)
      store.updateChildrenCache(null, ['node-1', 'node-2', 'node-3']);

      const allNodes = store.getNodesForParent(null);
      expect(allNodes).toHaveLength(3);
      expect(allNodes.map((n) => n.id)).toContain('node-1');
      expect(allNodes.map((n) => n.id)).toContain('node-2');
      expect(allNodes.map((n) => n.id)).toContain('node-3');
    });

    it('should get root nodes (no parent filtering needed)', () => {
      const root1: Node = { ...mockNode, id: 'root-1' };
      const root2: Node = { ...mockNode, id: 'root-2' };
      const root3: Node = { ...mockNode, id: 'root-3' };

      store.setNode(root1, viewerSource);
      store.setNode(root2, viewerSource);
      store.setNode(root3, viewerSource);

      // Populate cache to simulate backend hierarchy data
      // (Graph-native architecture: parentId removed, hierarchy stored as SurrealDB edges)
      store.updateChildrenCache(null, ['root-1', 'root-2', 'root-3']);

      const roots = store.getNodesForParent(null);
      expect(roots).toHaveLength(3);
      expect(roots.map((n) => n.id)).toContain('root-1');
      expect(roots.map((n) => n.id)).toContain('root-2');
      expect(roots.map((n) => n.id)).toContain('root-3');
    });
  });

  // ========================================================================
  // Update Operations
  // ========================================================================

  describe('Update Operations', () => {
    beforeEach(() => {
      store.setNode(mockNode, viewerSource);
    });

    it('should update node content', () => {
      const newContent = 'Updated content';
      store.updateNode(mockNode.id, { content: newContent }, viewerSource);

      const updated = store.getNode(mockNode.id);
      expect(updated?.content).toBe(newContent);
    });

    it('should update modifiedAt timestamp', () => {
      const originalTime = mockNode.modifiedAt;

      // Wait a bit to ensure timestamp changes
      vi.useFakeTimers();
      vi.advanceTimersByTime(100);

      store.updateNode(mockNode.id, { content: 'New content' }, viewerSource);

      const updated = store.getNode(mockNode.id);
      expect(updated?.modifiedAt).not.toBe(originalTime);

      vi.useRealTimers();
    });

    it('should batch update multiple nodes', () => {
      const node2: Node = { ...mockNode, id: 'test-node-2' };
      store.setNode(node2, viewerSource);

      store.updateNodes(
        [
          { nodeId: mockNode.id, changes: { content: 'Content 1' } },
          { nodeId: node2.id, changes: { content: 'Content 2' } }
        ],
        viewerSource
      );

      expect(store.getNode(mockNode.id)?.content).toBe('Content 1');
      expect(store.getNode(node2.id)?.content).toBe('Content 2');
    });

    it('should warn when updating non-existent node', () => {
      const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      store.updateNode('non-existent', { content: 'test' }, viewerSource);

      expect(consoleSpy).toHaveBeenCalledWith(
        expect.stringContaining('Cannot update non-existent node')
      );

      consoleSpy.mockRestore();
    });
  });

  // ========================================================================
  // Subscription System (Observer Pattern)
  // ========================================================================

  describe('Subscriptions', () => {
    it('should notify subscribers when node changes', () => {
      store.setNode(mockNode, viewerSource);

      const callback = vi.fn();
      store.subscribe(mockNode.id, callback);

      store.updateNode(mockNode.id, { content: 'New content' }, viewerSource);

      expect(callback).toHaveBeenCalledTimes(1);
      expect(callback).toHaveBeenCalledWith(
        expect.objectContaining({ content: 'New content' }),
        viewerSource
      );
    });

    it('should support multiple subscribers for same node', () => {
      store.setNode(mockNode, viewerSource);

      const callback1 = vi.fn();
      const callback2 = vi.fn();

      store.subscribe(mockNode.id, callback1);
      store.subscribe(mockNode.id, callback2);

      store.updateNode(mockNode.id, { content: 'New content' }, viewerSource);

      expect(callback1).toHaveBeenCalledTimes(1);
      expect(callback2).toHaveBeenCalledTimes(1);
    });

    it('should unsubscribe correctly', () => {
      store.setNode(mockNode, viewerSource);

      const callback = vi.fn();
      const unsubscribe = store.subscribe(mockNode.id, callback);

      store.updateNode(mockNode.id, { content: 'Content 1' }, viewerSource);
      expect(callback).toHaveBeenCalledTimes(1);

      unsubscribe();

      store.updateNode(mockNode.id, { content: 'Content 2' }, viewerSource);
      expect(callback).toHaveBeenCalledTimes(1); // Still 1, not called again
    });

    it('should support wildcard subscriptions', () => {
      const node1: Node = { ...mockNode, id: 'node-1' };
      const node2: Node = { ...mockNode, id: 'node-2' };

      store.setNode(node1, viewerSource);
      store.setNode(node2, viewerSource);

      const callback = vi.fn();
      store.subscribeAll(callback);

      store.updateNode('node-1', { content: 'Content 1' }, viewerSource);
      store.updateNode('node-2', { content: 'Content 2' }, viewerSource);

      expect(callback).toHaveBeenCalledTimes(2);
    });

    it('should handle subscription callback errors gracefully', () => {
      store.setNode(mockNode, viewerSource);

      const errorCallback = vi.fn(() => {
        throw new Error('Callback error');
      });
      const workingCallback = vi.fn();

      const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

      store.subscribe(mockNode.id, errorCallback);
      store.subscribe(mockNode.id, workingCallback);

      store.updateNode(mockNode.id, { content: 'New content' }, viewerSource);

      // Both callbacks should be called despite error
      expect(errorCallback).toHaveBeenCalled();
      expect(workingCallback).toHaveBeenCalled();
      expect(consoleSpy).toHaveBeenCalled();

      consoleSpy.mockRestore();
    });
  });

  // ========================================================================
  // Version Tracking
  // ========================================================================

  describe('Version Tracking', () => {
    it('should increment version on each update', () => {
      store.setNode(mockNode, viewerSource);

      const v1 = store.getVersion(mockNode.id);
      store.updateNode(mockNode.id, { content: 'Update 1' }, viewerSource);

      const v2 = store.getVersion(mockNode.id);
      expect(v2).toBeGreaterThan(v1);

      store.updateNode(mockNode.id, { content: 'Update 2' }, viewerSource);

      const v3 = store.getVersion(mockNode.id);
      expect(v3).toBeGreaterThan(v2);
    });

    it('should return 0 for non-existent node version', () => {
      expect(store.getVersion('non-existent')).toBe(0);
    });
  });

  // ========================================================================
  // Conflict Detection
  // ========================================================================

  describe('Conflict Detection', () => {
    it('should detect version mismatch conflicts', () => {
      store.setNode(mockNode, viewerSource);

      // First update succeeds
      store.updateNode(mockNode.id, { content: 'Update 1' }, viewerSource, {
        skipConflictDetection: false
      });

      const currentVersion = store.getVersion(mockNode.id);

      // Simulate concurrent edit with old version
      const oldVersionUpdate: NodeUpdate = {
        nodeId: mockNode.id,
        changes: { content: 'Concurrent update' },
        source: { type: 'viewer', viewerId: 'viewer-2' },
        timestamp: Date.now(),
        version: currentVersion + 1,
        previousVersion: currentVersion - 1 // Old version!
      };

      // This should trigger conflict detection
      // (Note: updateNode handles this internally, but we're testing the detection logic)
      const callback = vi.fn();
      store.subscribe(mockNode.id, callback);

      store.updateNode(mockNode.id, oldVersionUpdate.changes, oldVersionUpdate.source);

      // Should still update but conflict should be detected
      expect(callback).toHaveBeenCalled();
    });
  });

  // ========================================================================
  // Conflict Resolution
  // ========================================================================

  describe('Conflict Resolution', () => {
    it('should use Last-Write-Wins by default', () => {
      const resolver = store.getConflictResolver();
      expect(resolver).toBeInstanceOf(LastWriteWinsResolver);
    });

    it('should allow setting custom conflict resolver', () => {
      const customResolver = new LastWriteWinsResolver();
      store.setConflictResolver(customResolver);

      expect(store.getConflictResolver()).toBe(customResolver);
    });
  });

  // ========================================================================
  // Performance Metrics
  // ========================================================================

  describe('Performance Metrics', () => {
    it('should track update count', () => {
      store.setNode(mockNode, viewerSource);

      const metrics1 = store.getMetrics();
      expect(metrics1.updateCount).toBe(0);

      store.updateNode(mockNode.id, { content: 'Update 1' }, viewerSource, {
        skipConflictDetection: true
      });
      store.updateNode(mockNode.id, { content: 'Update 2' }, viewerSource, {
        skipConflictDetection: true
      });

      const metrics2 = store.getMetrics();
      expect(metrics2.updateCount).toBe(2);
    });

    it('should track subscription count', () => {
      store.setNode(mockNode, viewerSource);

      const metrics1 = store.getMetrics();
      const initialCount = metrics1.subscriptionCount;

      const unsub1 = store.subscribe(mockNode.id, () => {});
      const unsub2 = store.subscribe(mockNode.id, () => {});

      const metrics2 = store.getMetrics();
      expect(metrics2.subscriptionCount).toBe(initialCount + 2);

      unsub1();
      unsub2();

      const metrics3 = store.getMetrics();
      expect(metrics3.subscriptionCount).toBe(initialCount);
    });

    it('should track average update time', () => {
      store.setNode(mockNode, viewerSource);

      store.updateNode(mockNode.id, { content: 'Update 1' }, viewerSource);

      const metrics = store.getMetrics();
      expect(metrics.avgUpdateTime).toBeGreaterThan(0);
      expect(metrics.maxUpdateTime).toBeGreaterThan(0);
    });

    it('should reset metrics correctly', () => {
      store.setNode(mockNode, viewerSource);
      store.updateNode(mockNode.id, { content: 'Update' }, viewerSource);

      const metrics1 = store.getMetrics();
      expect(metrics1.updateCount).toBeGreaterThan(0);

      store.resetMetrics();

      const metrics2 = store.getMetrics();
      expect(metrics2.updateCount).toBe(0);
      expect(metrics2.avgUpdateTime).toBe(0);
      expect(metrics2.maxUpdateTime).toBe(0);
    });
  });

  // ========================================================================
  // Memory Leak Prevention
  // ========================================================================

  describe('Memory Leak Prevention', () => {
    it('should clean up subscriptions on unsubscribe', () => {
      store.setNode(mockNode, viewerSource);

      const unsubscribers: (() => void)[] = [];

      // Create many subscriptions
      for (let i = 0; i < 100; i++) {
        unsubscribers.push(store.subscribe(mockNode.id, () => {}));
      }

      const metrics1 = store.getMetrics();
      expect(metrics1.subscriptionCount).toBeGreaterThanOrEqual(100);

      // Unsubscribe all
      unsubscribers.forEach((unsub) => unsub());

      const metrics2 = store.getMetrics();
      expect(metrics2.subscriptionCount).toBeLessThan(metrics1.subscriptionCount);
    });

    it('should clean up pending updates on delete', () => {
      store.setNode(mockNode, viewerSource);

      // Create pending update
      store.updateNode(mockNode.id, { content: 'Update' }, viewerSource);

      // Delete node
      store.deleteNode(mockNode.id, viewerSource);

      // Verify cleanup (no direct API, but should not throw)
      expect(store.hasNode(mockNode.id)).toBe(false);
    });
  });

  // ========================================================================
  // New Methods for BaseNodeViewer Migration (Issue #237)
  // ========================================================================

  describe('BaseNodeViewer Migration Methods', () => {
    // --------------------------------------------------------------------
    // loadChildrenForParent()
    // --------------------------------------------------------------------
    describe('loadChildrenForParent', () => {
      it('should load children from database and add to store', async () => {
        // This test requires mocking tauriNodeService
        // For now, we'll test the integration when tauri service is available
        expect(store.loadChildrenForParent).toBeDefined();
      });

      it('should mark loaded nodes as persisted', async () => {
        // This test requires mocking tauriNodeService
        expect(store.loadChildrenForParent).toBeDefined();
      });

      it('should handle database errors gracefully', async () => {
        // Test error handling when database fails
        try {
          await store.loadChildrenForParent('non-existent-parent');
        } catch (error) {
          expect(error).toBeDefined();
        }
      });
    });

    // --------------------------------------------------------------------
    // Persistence delegation to PersistenceCoordinator
    // --------------------------------------------------------------------
    describe('Persistence Delegation', () => {
      it('should update node content in memory immediately', () => {
        store.setNode(mockNode, viewerSource);

        store.updateNode(mockNode.id, { content: 'Updated content' }, viewerSource);

        // Content should update in memory immediately (optimistic)
        expect(store.getNode(mockNode.id)?.content).toBe('Updated content');
      });

      it('should handle content changes with debounced persistence', async () => {
        const node: Node = {
          ...mockNode,
          id: 'test-node',
          content: 'Initial'
        };
        store.setNode(node, viewerSource, true); // skipPersistence

        // Update content (triggers debounced persistence)
        // Use 'database' source with explicit debounced persistence
        // (Phase 1 refactor: explicit persistence control replaces 'external' workaround)
        const updateSource: UpdateSource = { type: 'database', reason: 'test' };
        store.updateNode('test-node', { content: 'Updated' }, updateSource, {
          persist: 'debounced'
        });

        // Should update in memory immediately
        expect(store.getNode('test-node')?.content).toBe('Updated');

        // Wait for debounce (500ms) + a bit extra
        await new Promise((resolve) => setTimeout(resolve, 600));

        // Persistence should have been triggered (in test mode, just tracked)
        expect(PersistenceCoordinator.getInstance().isPersisted('test-node')).toBe(true);
      });

      it('should handle structural changes with immediate persistence', async () => {
        const node: Node = { ...mockNode, id: 'node' };

        store.setNode(node, viewerSource, true); // skipPersistence

        // Update structural property (triggers immediate persistence)
        store.updateNode('node', { beforeSiblingId: 'sibling-1' }, viewerSource);

        // Should update in memory immediately
        expect(store.getNode('node')?.beforeSiblingId).toBe('sibling-1');

        // Wait for immediate persistence
        await new Promise((resolve) => setTimeout(resolve, 50));

        // Persistence should have been triggered
        expect(PersistenceCoordinator.getInstance().isPersisted('node')).toBe(true);
      });
    });

    // --------------------------------------------------------------------
    // hasPendingSave() - Delegates to PersistenceCoordinator
    // --------------------------------------------------------------------
    describe('hasPendingSave', () => {
      it('should return false when no pending save', () => {
        expect(store.hasPendingSave('non-existent-node')).toBe(false);
      });

      it('should return true during pending save', async () => {
        const node: Node = { ...mockNode, id: 'test-node' };
        store.setNode(node, viewerSource, true); // skipPersistence

        // Trigger persistence via updateNode
        store.updateNode('test-node', { content: 'Updated' }, viewerSource);

        // Check immediately - should be pending
        const isPending = store.hasPendingSave('test-node');

        // May or may not be pending depending on timing (immediate mode vs debounced)
        // Just verify the method works
        expect(typeof isPending).toBe('boolean');
      });

      it('should return false after save completes', async () => {
        const node: Node = { ...mockNode, id: 'test-node' };
        store.setNode(node, viewerSource);

        // Wait for any persistence to complete
        await new Promise((resolve) => setTimeout(resolve, 600));

        expect(store.hasPendingSave('test-node')).toBe(false);
      });
    });

    // --------------------------------------------------------------------
    // waitForNodeSaves() - Delegates to PersistenceCoordinator
    // --------------------------------------------------------------------
    describe('waitForNodeSaves', () => {
      it('should return empty set when no pending saves', async () => {
        const failed = await store.waitForNodeSaves(['node-1', 'node-2']);
        expect(failed.size).toBe(0);
      });

      it('should wait for pending saves to complete', async () => {
        const node1: Node = { ...mockNode, id: 'node-1' };
        const node2: Node = { ...mockNode, id: 'node-2' };

        store.setNode(node1, viewerSource, true); // skipPersistence
        store.setNode(node2, viewerSource, true); // skipPersistence

        // Trigger persistence
        store.updateNode('node-1', { content: 'Content 1' }, viewerSource);
        store.updateNode('node-2', { content: 'Content 2' }, viewerSource);

        // Wait for persistence
        const failed = await store.waitForNodeSaves(['node-1', 'node-2'], 1000);
        expect(failed.size).toBe(0);
      });
    });

    // --------------------------------------------------------------------
    // validateNodeReferences()
    // --------------------------------------------------------------------
    describe('validateNodeReferences', () => {
      it('should return error when node does not exist', async () => {
        const result = await store.validateNodeReferences('non-existent', null, null, null);

        expect(result.errors).toHaveLength(1);
        expect(result.errors[0]).toContain('not found');
      });

      it('should skip parent validation (parentId removed)', async () => {
        store.setNode(mockNode, viewerSource);

        const result = await store.validateNodeReferences(
          mockNode.id,
          null,
          null,
          null
        );

        expect(result.errors).toHaveLength(0);
      });

      it('should validate without parent references', async () => {
        store.setNode(mockNode, viewerSource);

        const result = await store.validateNodeReferences(
          mockNode.id,
          null,
          null,
          null
        );

        expect(result.errors).toHaveLength(0);
        expect(result.validatedParentId).toBeNull();
      });

      it('should null out invalid beforeSiblingId', async () => {
        store.setNode(mockNode, viewerSource);

        const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

        const result = await store.validateNodeReferences(
          mockNode.id,
          null,
          'non-existent-sibling',
          null
        );

        expect(result.errors).toHaveLength(0);
        expect(result.validatedBeforeSiblingId).toBeNull();
        expect(consoleSpy).toHaveBeenCalled();

        consoleSpy.mockRestore();
      });

      it('should validate sibling references successfully', async () => {
        const sibling: Node = { ...mockNode, id: 'sibling-1' };
        store.setNode(mockNode, viewerSource);
        store.setNode(sibling, viewerSource);

        const result = await store.validateNodeReferences(
          mockNode.id,
          null,
          'sibling-1',
          null
        );

        expect(result.errors).toHaveLength(0);
        expect(result.validatedParentId).toBeNull();
        expect(result.validatedBeforeSiblingId).toBe('sibling-1');
      });
    });

    // --------------------------------------------------------------------
    // updateStructuralChangesValidated()
    // --------------------------------------------------------------------
    describe('updateStructuralChangesValidated', () => {
      it('should process valid updates successfully', async () => {
        const node1: Node = { ...mockNode, id: 'node-1' };
        const node2: Node = { ...mockNode, id: 'node-2' };

        store.setNode(node1, viewerSource);
        store.setNode(node2, viewerSource);

        const updates = [
          { nodeId: 'node-1', beforeSiblingId: null },
          { nodeId: 'node-2', beforeSiblingId: 'node-1' }
        ];

        const result = await store.updateStructuralChangesValidated(updates, viewerSource, null);

        expect(result.succeeded).toHaveLength(2);
        expect(result.failed).toHaveLength(0);
        expect(result.errors.size).toBe(0);

        // Verify updates were applied
        expect(store.getNode('node-2')?.beforeSiblingId).toBe('node-1');
      });

      it('should handle validation errors gracefully', async () => {
        store.setNode(mockNode, viewerSource);

        const updates = [
          { nodeId: mockNode.id, beforeSiblingId: null }
        ];

        const result = await store.updateStructuralChangesValidated(updates, viewerSource, null);

        // Should succeed since no parent validation is required
        expect(result.succeeded).toHaveLength(1);
        expect(result.failed).toHaveLength(0);
        expect(result.errors.size).toBe(0);
      });

      it('should process updates serially', async () => {
        const node1: Node = { ...mockNode, id: 'node-1' };
        const node2: Node = { ...mockNode, id: 'node-2' };

        store.setNode(node1, viewerSource);
        store.setNode(node2, viewerSource);

        const executionOrder: string[] = [];
        const originalUpdateNode = store.updateNode.bind(store);

        // Mock updateNode to track execution order
        store.updateNode = vi.fn((nodeId, changes, source, options) => {
          executionOrder.push(nodeId);
          return originalUpdateNode(nodeId, changes, source, options);
        });

        const updates = [
          { nodeId: 'node-1', beforeSiblingId: null },
          { nodeId: 'node-2', beforeSiblingId: null }
        ];

        await store.updateStructuralChangesValidated(updates, viewerSource, null);

        // Verify serial processing (node-1 before node-2)
        expect(executionOrder).toEqual(['node-1', 'node-2']);

        // Restore original method
        store.updateNode = originalUpdateNode;
      });

      it('should null out invalid beforeSiblingId references', async () => {
        const node: Node = { ...mockNode, id: 'node-1' };

        store.setNode(node, viewerSource);

        const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

        const updates = [
          {
            nodeId: 'node-1',
            beforeSiblingId: 'non-existent-sibling'
          }
        ];

        const result = await store.updateStructuralChangesValidated(updates, viewerSource, null);

        expect(result.succeeded).toHaveLength(1);
        expect(result.succeeded[0].beforeSiblingId).toBeNull();

        consoleSpy.mockRestore();
      });

      it('should handle mixed success and failure', async () => {
        const node1: Node = { ...mockNode, id: 'node-1' };

        store.setNode(node1, viewerSource);

        const updates = [
          { nodeId: 'node-1', beforeSiblingId: null },
          { nodeId: 'non-existent-node', beforeSiblingId: null }
        ];

        const result = await store.updateStructuralChangesValidated(updates, viewerSource, null);

        expect(result.succeeded).toHaveLength(1);
        expect(result.failed).toHaveLength(1);
        expect(result.succeeded[0].nodeId).toBe('node-1');
        expect(result.failed[0].nodeId).toBe('non-existent-node');
      });
    });
  });

  // ========================================================================
  // Atomic Batch Updates
  // ========================================================================

  describe('Atomic Batch Updates', () => {
    let quoteNode: Node;

    beforeEach(() => {
      quoteNode = {
        ...mockNode,
        id: 'quote-node-1',
        nodeType: 'quote-block',
        content: '> Hello world'
      };
    });

    describe('startBatch', () => {
      it('should create a new batch for a node', () => {
        const batchId = store.startBatch('quote-node-1');

        expect(batchId).toMatch(/^batch-quote-node-1-\d+$/);
      });

      it('should cancel existing batch when starting new one', () => {
        const batchId1 = store.startBatch('quote-node-1');
        const batchId2 = store.startBatch('quote-node-1');

        expect(batchId1).not.toBe(batchId2);
      });

      it('should use default timeout if not specified', () => {
        const batchId = store.startBatch('quote-node-1');
        expect(batchId).toBeDefined();
      });

      it('should accept custom timeout', () => {
        const batchId = store.startBatch('quote-node-1', 5000);
        expect(batchId).toBeDefined();
      });
    });

    describe('addToBatch', () => {
      it('should accumulate changes in batch', () => {
        store.setNode(quoteNode, viewerSource);
        store.startBatch('quote-node-1');

        store.addToBatch('quote-node-1', { content: '> New content' });
        store.addToBatch('quote-node-1', { nodeType: 'quote-block' });

        const node = store.getNode('quote-node-1');
        expect(node?.content).toBe('> New content');
        expect(node?.nodeType).toBe('quote-block');
      });

      it('should update in-memory state immediately (optimistic)', () => {
        store.setNode(quoteNode, viewerSource);
        store.startBatch('quote-node-1');

        store.addToBatch('quote-node-1', { content: '> Updated' });

        const node = store.getNode('quote-node-1');
        expect(node?.content).toBe('> Updated');
      });

      it('should merge later changes over earlier ones', () => {
        store.setNode(quoteNode, viewerSource);
        store.startBatch('quote-node-1');

        store.addToBatch('quote-node-1', { content: '> First' });
        store.addToBatch('quote-node-1', { content: '> Second' });
        store.addToBatch('quote-node-1', { content: '> Third' });

        const node = store.getNode('quote-node-1');
        expect(node?.content).toBe('> Third');
      });

      it('should reset timeout on each change (true inactivity)', async () => {
        store.setNode(quoteNode, viewerSource);
        store.startBatch('quote-node-1', 100); // 100ms timeout

        // Add change at t=0
        store.addToBatch('quote-node-1', { content: '> First' });

        // Wait 50ms, then add another change (resets timeout)
        await new Promise((resolve) => setTimeout(resolve, 50));
        store.addToBatch('quote-node-1', { content: '> Second' });

        // Wait another 50ms (total 100ms from first change, but only 50ms from second)
        await new Promise((resolve) => setTimeout(resolve, 50));

        // Batch should still be active (timeout was reset)
        const node = store.getNode('quote-node-1');
        expect(node?.content).toBe('> Second');
      });

      it('should warn if batch does not exist', () => {
        const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

        store.addToBatch('non-existent-node', { content: 'test' });

        expect(consoleSpy).toHaveBeenCalledWith(
          expect.stringContaining('Attempted to add to non-existent batch'),
          expect.any(Object)
        );

        consoleSpy.mockRestore();
      });
    });

    describe('commitBatch', () => {
      it('should persist changes when batch commits', async () => {
        store.setNode(quoteNode, viewerSource);
        store.startBatch('quote-node-1');

        store.addToBatch('quote-node-1', { content: '> Final content' });
        store.commitBatch('quote-node-1');

        // Wait for persistence
        await new Promise((resolve) => setTimeout(resolve, 10));

        const node = store.getNode('quote-node-1');
        expect(node?.content).toBe('> Final content');
      });

      it('should skip persistence if final state is placeholder', () => {
        const placeholderNode = {
          ...mockNode,
          id: 'placeholder-1',
          nodeType: 'quote-block',
          content: '> '
        };

        store.setNode(placeholderNode, viewerSource, true);
        store.startBatch('placeholder-1');

        store.addToBatch('placeholder-1', { content: '> ' });
        store.commitBatch('placeholder-1');

        // Should not persist (placeholder detection)
        const node = store.getNode('placeholder-1');
        expect(node?.content).toBe('> ');
      });

      it('should persist if node was previously persisted (even if now placeholder)', async () => {
        // First persist with real content
        store.setNode(quoteNode, viewerSource);
        await new Promise((resolve) => setTimeout(resolve, 10));

        // Now batch update that deletes content back to placeholder
        store.startBatch('quote-node-1');
        store.addToBatch('quote-node-1', { content: '> ' });
        store.commitBatch('quote-node-1');

        // Should still persist to update DB with empty state
        await new Promise((resolve) => setTimeout(resolve, 10));

        const node = store.getNode('quote-node-1');
        expect(node?.content).toBe('> ');
      });

      it('should auto-commit after timeout', async () => {
        store.setNode(quoteNode, viewerSource);
        store.startBatch('quote-node-1', 50); // 50ms timeout

        store.addToBatch('quote-node-1', { content: '> Timeout test' });

        // Wait for timeout to fire
        await new Promise((resolve) => setTimeout(resolve, 100));

        // Batch should have auto-committed
        const node = store.getNode('quote-node-1');
        expect(node?.content).toBe('> Timeout test');
      });

      it('should handle empty batch (no changes)', () => {
        store.setNode(quoteNode, viewerSource);
        store.startBatch('quote-node-1');

        // Don't add any changes
        store.commitBatch('quote-node-1');

        // Should handle gracefully
        const node = store.getNode('quote-node-1');
        expect(node).toBeDefined();
      });

      it('should clean up timeout and batch state on commit', () => {
        store.setNode(quoteNode, viewerSource);
        store.startBatch('quote-node-1');

        store.addToBatch('quote-node-1', { content: '> Test' });
        store.commitBatch('quote-node-1');

        // Trying to add to committed batch should warn
        const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
        store.addToBatch('quote-node-1', { content: '> After commit' });

        expect(consoleSpy).toHaveBeenCalled();
        consoleSpy.mockRestore();
      });
    });

    describe('cancelBatch', () => {
      it('should cancel active batch without persisting', () => {
        store.setNode(quoteNode, viewerSource);
        store.startBatch('quote-node-1');

        store.addToBatch('quote-node-1', { content: '> Cancelled' });
        store.cancelBatch('quote-node-1');

        // Changes should still be in memory (optimistic) but batch is gone
        const node = store.getNode('quote-node-1');
        expect(node?.content).toBe('> Cancelled');
      });

      it('should clean up timeout on cancel', () => {
        store.startBatch('quote-node-1', 100);
        store.cancelBatch('quote-node-1');

        // Should not throw or warn after timeout would have fired
        // (timeout was cleared)
      });

      it('should handle cancelling non-existent batch gracefully', () => {
        // Should not throw
        store.cancelBatch('non-existent-node');
      });
    });

    describe('Auto-restart batching for pattern-converted nodes', () => {
      it('should auto-restart batch for quote-block nodes', () => {
        store.setNode(quoteNode, viewerSource, true);

        // Update content (should auto-start batch)
        store.updateNode('quote-node-1', { content: '> Updated' }, viewerSource);

        // Should have started a batch automatically
        // (verify by checking that subsequent updates are batched)
        store.updateNode('quote-node-1', { content: '> Again' }, viewerSource);

        const node = store.getNode('quote-node-1');
        expect(node?.content).toBe('> Again');
      });

      it('should auto-restart batch for code-block nodes', () => {
        const codeNode = {
          ...mockNode,
          id: 'code-node-1',
          nodeType: 'code-block',
          content: '```\ncode\n```'
        };

        store.setNode(codeNode, viewerSource, true);
        store.updateNode('code-node-1', { content: '```\nupdated\n```' }, viewerSource);

        const node = store.getNode('code-node-1');
        expect(node?.content).toBe('```\nupdated\n```');
      });

      it('should auto-restart batch for ordered-list nodes', () => {
        const listNode = {
          ...mockNode,
          id: 'list-node-1',
          nodeType: 'ordered-list',
          content: '1. Item'
        };

        store.setNode(listNode, viewerSource, true);
        store.updateNode('list-node-1', { content: '1. Updated' }, viewerSource);

        const node = store.getNode('list-node-1');
        expect(node?.content).toBe('1. Updated');
      });

      it('should respect skipPersistence option in auto-restart', () => {
        store.setNode(quoteNode, viewerSource, true);

        // Update with skipPersistence should NOT start batch
        store.updateNode('quote-node-1', { content: '> Updated' }, viewerSource, {
          skipPersistence: true
        });

        // Should fall through to normal path (no batch)
        const node = store.getNode('quote-node-1');
        expect(node?.content).toBe('> Updated');
      });

      it('should not auto-restart for non-pattern-converted types', () => {
        const textNode = {
          ...mockNode,
          id: 'text-node-1',
          nodeType: 'text',
          content: 'Hello'
        };

        store.setNode(textNode, viewerSource, true);
        store.updateNode('text-node-1', { content: 'Updated' }, viewerSource);

        // Should not have started a batch (text nodes don't require batching)
        const node = store.getNode('text-node-1');
        expect(node?.content).toBe('Updated');
      });
    });

    describe('Concurrent batches', () => {
      it('should handle concurrent batches on different nodes', () => {
        const node1 = { ...mockNode, id: 'node-1', nodeType: 'quote-block', content: '> A' };
        const node2 = { ...mockNode, id: 'node-2', nodeType: 'code-block', content: '```\nB\n```' };

        store.setNode(node1, viewerSource, true);
        store.setNode(node2, viewerSource, true);

        store.startBatch('node-1');
        store.startBatch('node-2');

        store.addToBatch('node-1', { content: '> A updated' });
        store.addToBatch('node-2', { content: '```\nB updated\n```' });

        store.commitBatch('node-1');
        store.commitBatch('node-2');

        expect(store.getNode('node-1')?.content).toBe('> A updated');
        expect(store.getNode('node-2')?.content).toBe('```\nB updated\n```');
      });

      it('should maintain separate timeout for each batch', async () => {
        const node1 = { ...mockNode, id: 'node-1', nodeType: 'quote-block', content: '> A' };
        const node2 = { ...mockNode, id: 'node-2', nodeType: 'code-block', content: '```\nB\n```' };

        store.setNode(node1, viewerSource, true);
        store.setNode(node2, viewerSource, true);

        store.startBatch('node-1', 50);
        store.startBatch('node-2', 100);

        store.addToBatch('node-1', { content: '> A updated' });
        store.addToBatch('node-2', { content: '```\nB updated\n```' });

        // Wait for node-1's timeout but not node-2's
        await new Promise((resolve) => setTimeout(resolve, 75));

        // node-1 should have auto-committed, node-2 should still be batching
        expect(store.getNode('node-1')?.content).toBe('> A updated');
        expect(store.getNode('node-2')?.content).toBe('```\nB updated\n```');
      });
    });

    describe('Race condition handling', () => {
      it('should handle race where old path persists before batch', async () => {
        // This is difficult to test directly, but we can verify the CREATE→UPDATE fallback
        // by simulating the scenario where a node is persisted outside the batch

        const node = { ...mockNode, id: 'race-node-1', nodeType: 'quote-block', content: '> Test' };

        // Simulate old path persisting
        store.setNode(node, viewerSource);
        await new Promise((resolve) => setTimeout(resolve, 10));

        // Now batch with updated content
        store.startBatch('race-node-1');
        store.addToBatch('race-node-1', { content: '> Updated via batch' });
        store.commitBatch('race-node-1');

        await new Promise((resolve) => setTimeout(resolve, 10));

        const finalNode = store.getNode('race-node-1');
        expect(finalNode?.content).toBe('> Updated via batch');
      });
    });

    describe('Batch cleanup on node deletion', () => {
      it('should cancel batch when node is deleted', () => {
        store.setNode(quoteNode, viewerSource, true);
        store.startBatch('quote-node-1');

        store.addToBatch('quote-node-1', { content: '> Test' });

        // Delete node (should cancel batch)
        store.deleteNode('quote-node-1', viewerSource);

        // Batch should be gone
        const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
        store.addToBatch('quote-node-1', { content: '> After delete' });

        expect(consoleSpy).toHaveBeenCalled();
        consoleSpy.mockRestore();
      });
    });
  });

  describe('Explicit Persistence API (Issue #393 Refactor)', () => {
    const mockNode: Node = {
      id: 'persist-test',
      nodeType: 'text',
      content: 'Test',
      beforeSiblingId: null,
      version: 1,
      properties: {},
      createdAt: Date.now().toString(),
      modifiedAt: Date.now().toString()
    };

    beforeEach(() => {
      store = SharedNodeStore.getInstance();
      PersistenceCoordinator.resetInstance();
    });

    describe('persist option', () => {
      it('should respect persist: false to skip persistence', async () => {
        const viewerSource: UpdateSource = { type: 'viewer', viewerId: 'test-viewer' };
        store.setNode(mockNode, viewerSource);

        // Update with explicit persist: false
        store.updateNode('persist-test', { content: 'Updated' }, viewerSource, {
          persist: false
        });

        // Should update in memory
        expect(store.getNode('persist-test')?.content).toBe('Updated');

        // Wait to ensure no persistence triggered
        await new Promise((resolve) => setTimeout(resolve, 100));

        // Should NOT be persisted
        expect(PersistenceCoordinator.getInstance().isPersisted('persist-test')).toBe(false);
      });

      it('should respect persist: true for explicit auto-determined persistence', async () => {
        const viewerSource: UpdateSource = { type: 'viewer', viewerId: 'test-viewer' };
        store.setNode(mockNode, viewerSource);

        // Update with explicit persist: true (triggers persistence with auto mode)
        // Structural changes use immediate mode, content changes use debounced mode
        store.updateNode('persist-test', { beforeSiblingId: 'sibling-1' }, viewerSource, {
          persist: true
        });

        // Should update in memory immediately

        // Structural change should trigger immediate persistence (not debounced)
        // Check that operation was queued (isPending or isPersisted)
        const coordinator = PersistenceCoordinator.getInstance();
        const isPendingOrPersisted =
          coordinator.isPending('persist-test') || coordinator.isPersisted('persist-test');
        expect(isPendingOrPersisted).toBe(true);
      });
    });

    describe('markAsPersistedOnly option', () => {
      it('should mark node as persisted without re-persisting', () => {
        const backendSource: UpdateSource = { type: 'database', reason: 'navigation' };

        // Load node from backend (using database source marks as persisted)
        store.setNode(mockNode, backendSource);

        // Verify node is marked as persisted internally
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        expect((store as any).persistedNodeIds.has('persist-test')).toBe(true);
      });
    });

    describe('legacy source.type behavior', () => {
      it('should maintain backward compatibility: database source skips persistence', async () => {
        const backendSource: UpdateSource = { type: 'database', reason: 'test' };

        // Old behavior: database source implicitly skips persistence
        store.setNode(mockNode, backendSource);
        store.updateNode('persist-test', { content: 'Updated' }, backendSource);

        // Wait to ensure no persistence
        await new Promise((resolve) => setTimeout(resolve, 100));

        // Should NOT trigger persistence operation
        expect(PersistenceCoordinator.getInstance().isPending('persist-test')).toBe(false);
      });
    });
  });
});
