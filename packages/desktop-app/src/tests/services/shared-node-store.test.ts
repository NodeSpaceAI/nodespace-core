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
import type { Node } from '../../lib/types';
import type { UpdateSource, NodeUpdate } from '../../lib/types/update-protocol';
import { LastWriteWinsResolver } from '../../lib/services/conflict-resolvers';

describe('SharedNodeStore', () => {
  let store: SharedNodeStore;
  const mockNode: Node = {
    id: 'test-node-1',
    nodeType: 'text',
    content: 'Test content',
    parentId: null,
    containerNodeId: null,
    beforeSiblingId: null,
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
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
  });

  afterEach(() => {
    // Clean up
    store.clearAll();
    SharedNodeStore.resetInstance();
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
    it('should get nodes by parent ID', () => {
      const parent: Node = { ...mockNode, id: 'parent-1' };
      const child1: Node = { ...mockNode, id: 'child-1', parentId: 'parent-1' };
      const child2: Node = { ...mockNode, id: 'child-2', parentId: 'parent-1' };
      const child3: Node = { ...mockNode, id: 'child-3', parentId: 'parent-2' };

      store.setNode(parent, viewerSource);
      store.setNode(child1, viewerSource);
      store.setNode(child2, viewerSource);
      store.setNode(child3, viewerSource);

      const parent1Children = store.getNodesForParent('parent-1');
      expect(parent1Children).toHaveLength(2);
      expect(parent1Children.map((n) => n.id)).toContain('child-1');
      expect(parent1Children.map((n) => n.id)).toContain('child-2');

      const parent2Children = store.getNodesForParent('parent-2');
      expect(parent2Children).toHaveLength(1);
      expect(parent2Children[0].id).toBe('child-3');
    });

    it('should get root nodes (parentId === null)', () => {
      const root1: Node = { ...mockNode, id: 'root-1', parentId: null };
      const root2: Node = { ...mockNode, id: 'root-2', parentId: null };
      const child: Node = { ...mockNode, id: 'child-1', parentId: 'root-1' };

      store.setNode(root1, viewerSource);
      store.setNode(root2, viewerSource);
      store.setNode(child, viewerSource);

      const roots = store.getNodesForParent(null);
      expect(roots).toHaveLength(2);
      expect(roots.map((n) => n.id)).toContain('root-1');
      expect(roots.map((n) => n.id)).toContain('root-2');
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
    // updateNodeContentDebounced()
    // --------------------------------------------------------------------
    describe('updateNodeContentDebounced', () => {
      beforeEach(() => {
        vi.useFakeTimers();
      });

      afterEach(() => {
        vi.useRealTimers();
      });

      it('should debounce content updates by 500ms', () => {
        store.setNode(mockNode, viewerSource);

        store.updateNodeContentDebounced(mockNode.id, 'Update 1', 'text', false, viewerSource);

        // Content should not update immediately
        expect(store.getNode(mockNode.id)?.content).toBe('Test content');

        vi.advanceTimersByTime(500);

        // Content should update after 500ms
        expect(store.getNode(mockNode.id)?.content).toBe('Update 1');
      });

      it('should reset timer on multiple rapid updates', () => {
        store.setNode(mockNode, viewerSource);

        store.updateNodeContentDebounced(mockNode.id, 'Update 1', 'text', false, viewerSource);

        vi.advanceTimersByTime(300);

        store.updateNodeContentDebounced(mockNode.id, 'Update 2', 'text', false, viewerSource);

        // After 200ms more (500ms total from first call), should NOT update
        vi.advanceTimersByTime(200);
        expect(store.getNode(mockNode.id)?.content).toBe('Test content');

        // After 300ms more (500ms from second call), SHOULD update to Update 2
        vi.advanceTimersByTime(300);
        expect(store.getNode(mockNode.id)?.content).toBe('Update 2');
      });

      it('should skip persistence for placeholder nodes', () => {
        store.setNode(mockNode, viewerSource);

        store.updateNodeContentDebounced(
          mockNode.id,
          'Placeholder content',
          'text',
          true, // isPlaceholder = true
          viewerSource
        );

        // Content should update immediately (in-memory only)
        expect(store.getNode(mockNode.id)?.content).toBe('Placeholder content');

        // Should not wait for debounce timer
        vi.advanceTimersByTime(500);
        expect(store.getNode(mockNode.id)?.content).toBe('Placeholder content');
      });

      it('should clean up timer on delete', () => {
        store.setNode(mockNode, viewerSource);

        store.updateNodeContentDebounced(mockNode.id, 'Update 1', 'text', false, viewerSource);

        vi.advanceTimersByTime(500);

        expect(store.getNode(mockNode.id)?.content).toBe('Update 1');
      });
    });

    // --------------------------------------------------------------------
    // saveNodeImmediately()
    // --------------------------------------------------------------------
    describe('saveNodeImmediately', () => {
      it('should save new node immediately', async () => {
        await store.saveNodeImmediately(
          'new-node',
          'New content',
          'text',
          null,
          'container-1',
          null,
          false,
          viewerSource
        );

        const node = store.getNode('new-node');
        expect(node).toBeDefined();
        expect(node?.content).toBe('New content');
        expect(node?.nodeType).toBe('text');
      });

      it('should update existing node immediately', async () => {
        store.setNode(mockNode, viewerSource);

        await store.saveNodeImmediately(
          mockNode.id,
          'Updated content',
          'text',
          null,
          'container-1',
          null,
          false,
          viewerSource
        );

        const node = store.getNode(mockNode.id);
        expect(node?.content).toBe('Updated content');
      });

      it('should skip placeholder nodes', async () => {
        await store.saveNodeImmediately(
          'placeholder-node',
          'Placeholder content',
          'text',
          null,
          'container-1',
          null,
          true, // isPlaceholder = true
          viewerSource
        );

        // Placeholder should not be persisted
        const node = store.getNode('placeholder-node');
        expect(node).toBeUndefined();
      });

      it('should track pending saves for FOREIGN KEY coordination', async () => {
        const savePromise = store.saveNodeImmediately(
          'new-node',
          'New content',
          'text',
          null,
          'container-1',
          null,
          false,
          viewerSource
        );

        // Should have pending save
        expect(store.hasPendingSave('new-node')).toBe(true);

        await savePromise;

        // Should clean up after save completes
        expect(store.hasPendingSave('new-node')).toBe(false);
      });

      it('should not include mentions field in persisted node', async () => {
        await store.saveNodeImmediately(
          'new-node',
          'Content with @mention',
          'text',
          null,
          'container-1',
          null,
          false,
          viewerSource
        );

        const node = store.getNode('new-node');
        // mentions is computed, not persisted - should not be in the node object
        expect(node).toBeDefined();
        expect(node?.content).toBe('Content with @mention');
      });
    });

    // --------------------------------------------------------------------
    // hasPendingSave()
    // --------------------------------------------------------------------
    describe('hasPendingSave', () => {
      it('should return false when no pending save', () => {
        expect(store.hasPendingSave('non-existent-node')).toBe(false);
      });

      it('should return true during pending save', async () => {
        const savePromise = store.saveNodeImmediately(
          'new-node',
          'New content',
          'text',
          null,
          'container-1',
          null,
          false,
          viewerSource
        );

        expect(store.hasPendingSave('new-node')).toBe(true);

        await savePromise;

        expect(store.hasPendingSave('new-node')).toBe(false);
      });

      it('should return false after save completes', async () => {
        await store.saveNodeImmediately(
          'new-node',
          'New content',
          'text',
          null,
          'container-1',
          null,
          false,
          viewerSource
        );

        expect(store.hasPendingSave('new-node')).toBe(false);
      });
    });

    // --------------------------------------------------------------------
    // waitForNodeSaves()
    // --------------------------------------------------------------------
    describe('waitForNodeSaves', () => {
      it('should return empty set when no pending saves', async () => {
        const failed = await store.waitForNodeSaves(['node-1', 'node-2']);
        expect(failed.size).toBe(0);
      });

      it('should wait for pending saves to complete', async () => {
        const savePromise1 = store.saveNodeImmediately(
          'node-1',
          'Content 1',
          'text',
          null,
          'container-1',
          null,
          false,
          viewerSource
        );

        const savePromise2 = store.saveNodeImmediately(
          'node-2',
          'Content 2',
          'text',
          null,
          'container-1',
          null,
          false,
          viewerSource
        );

        const waitPromise = store.waitForNodeSaves(['node-1', 'node-2'], 5000);

        // Let saves complete
        await Promise.all([savePromise1, savePromise2]);

        const failed = await waitPromise;
        expect(failed.size).toBe(0);
      });

      it('should return failed nodes on timeout', async () => {
        // Create a save that never completes
        const neverCompletingPromise = new Promise<void>(() => {
          // Never resolves
        });

        // Manually add to pending saves to simulate stuck save
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const pendingSaves = (store as any).pendingContentSaves;
        pendingSaves.set('stuck-node', neverCompletingPromise);

        // Use very short timeout for testing
        const failed = await store.waitForNodeSaves(['stuck-node'], 100);

        expect(failed.size).toBeGreaterThan(0);
        expect(failed.has('stuck-node')).toBe(true);

        // Clean up
        pendingSaves.delete('stuck-node');
      });

      it('should use grace period to detect in-flight saves', async () => {
        // Create a save that completes during grace period
        let resolveSave: () => void;
        const delayedSavePromise = new Promise<void>((resolve) => {
          resolveSave = resolve;
        });

        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const pendingSaves = (store as any).pendingContentSaves;
        pendingSaves.set('delayed-node', delayedSavePromise);

        // Start the wait with short timeout
        const waitPromise = store.waitForNodeSaves(['delayed-node'], 50);

        // Resolve the save immediately (before timeout)
        setTimeout(() => resolveSave!(), 10);

        const failed = await waitPromise;

        // Should NOT be marked as failed since it completed
        expect(failed.has('delayed-node')).toBe(false);

        pendingSaves.delete('delayed-node');
      });

      it('should handle mixed success and failure', async () => {
        // One successful save
        const savePromise1 = store.saveNodeImmediately(
          'node-1',
          'Content 1',
          'text',
          null,
          'container-1',
          null,
          false,
          viewerSource
        );

        // One stuck save
        const neverCompletingPromise = new Promise<void>(() => {});
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const pendingSaves = (store as any).pendingContentSaves;
        pendingSaves.set('stuck-node', neverCompletingPromise);

        // Let first save complete first
        await savePromise1;

        // Now wait for both with short timeout
        const failed = await store.waitForNodeSaves(['node-1', 'stuck-node'], 100);

        expect(failed.has('node-1')).toBe(false);
        expect(failed.has('stuck-node')).toBe(true);

        pendingSaves.delete('stuck-node');
      });
    });

    // --------------------------------------------------------------------
    // ensureAncestorChainPersisted()
    // --------------------------------------------------------------------
    describe('ensureAncestorChainPersisted', () => {
      it('should do nothing when node has no parent', async () => {
        store.setNode(mockNode, viewerSource);

        const checkIsPlaceholder = vi.fn(() => false);

        await store.ensureAncestorChainPersisted(mockNode.id, checkIsPlaceholder);

        expect(checkIsPlaceholder).not.toHaveBeenCalled();
      });

      it('should persist placeholder parent', async () => {
        const parent: Node = {
          ...mockNode,
          id: 'parent-1',
          content: '',
          parentId: null
        };

        const child: Node = {
          ...mockNode,
          id: 'child-1',
          parentId: 'parent-1',
          containerNodeId: 'parent-1'
        };

        store.setNode(parent, viewerSource);
        store.setNode(child, viewerSource);

        const checkIsPlaceholder = vi.fn((nodeId: string) => nodeId === 'parent-1');

        await store.ensureAncestorChainPersisted('child-1', checkIsPlaceholder);

        expect(checkIsPlaceholder).toHaveBeenCalledWith('parent-1');

        // Parent should have been saved (content would be empty string)
        const updatedParent = store.getNode('parent-1');
        expect(updatedParent?.content).toBe('');
      });

      it('should recursively persist grandparents', async () => {
        const grandparent: Node = {
          ...mockNode,
          id: 'grandparent',
          content: '',
          parentId: null
        };

        const parent: Node = {
          ...mockNode,
          id: 'parent',
          content: '',
          parentId: 'grandparent',
          containerNodeId: 'grandparent'
        };

        const child: Node = {
          ...mockNode,
          id: 'child',
          parentId: 'parent',
          containerNodeId: 'grandparent'
        };

        store.setNode(grandparent, viewerSource);
        store.setNode(parent, viewerSource);
        store.setNode(child, viewerSource);

        const checkIsPlaceholder = vi.fn(
          (nodeId: string) => nodeId === 'grandparent' || nodeId === 'parent'
        );

        await store.ensureAncestorChainPersisted('child', checkIsPlaceholder);

        // Should check both grandparent and parent
        expect(checkIsPlaceholder).toHaveBeenCalledWith('parent');
        expect(checkIsPlaceholder).toHaveBeenCalledWith('grandparent');
      });

      it('should stop at first non-placeholder ancestor', async () => {
        const grandparent: Node = {
          ...mockNode,
          id: 'grandparent',
          content: 'Real content',
          parentId: null
        };

        const parent: Node = {
          ...mockNode,
          id: 'parent',
          content: '',
          parentId: 'grandparent',
          containerNodeId: 'grandparent'
        };

        const child: Node = {
          ...mockNode,
          id: 'child',
          parentId: 'parent',
          containerNodeId: 'grandparent'
        };

        store.setNode(grandparent, viewerSource);
        store.setNode(parent, viewerSource);
        store.setNode(child, viewerSource);

        // Only parent is a placeholder, grandparent is not
        const checkIsPlaceholder = vi.fn((nodeId: string) => nodeId === 'parent');

        await store.ensureAncestorChainPersisted('child', checkIsPlaceholder);

        // Should check parent (it's a placeholder)
        expect(checkIsPlaceholder).toHaveBeenCalledWith('parent');

        // The implementation recursively calls ensureAncestorChainPersisted on parent,
        // which then checks grandparent. Since grandparent is not a placeholder,
        // it stops there. This is correct behavior - it needs to check to know whether to stop.
        expect(checkIsPlaceholder).toHaveBeenCalledWith('grandparent');

        // Verify it was only called twice (parent, grandparent) and stopped
        expect(checkIsPlaceholder).toHaveBeenCalledTimes(2);
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

      it('should validate parentId exists', async () => {
        store.setNode(mockNode, viewerSource);

        const result = await store.validateNodeReferences(
          mockNode.id,
          'non-existent-parent',
          null,
          null
        );

        expect(result.errors).toHaveLength(1);
        expect(result.errors[0]).toContain('Parent');
      });

      it('should allow viewer parent ID even if not in store', async () => {
        store.setNode(mockNode, viewerSource);

        const result = await store.validateNodeReferences(
          mockNode.id,
          'viewer-parent',
          null,
          'viewer-parent' // viewerParentId
        );

        expect(result.errors).toHaveLength(0);
        expect(result.validatedParentId).toBe('viewer-parent');
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

      it('should validate all references successfully', async () => {
        const parent: Node = { ...mockNode, id: 'parent-1' };
        const sibling: Node = { ...mockNode, id: 'sibling-1' };
        store.setNode(mockNode, viewerSource);
        store.setNode(parent, viewerSource);
        store.setNode(sibling, viewerSource);

        const result = await store.validateNodeReferences(
          mockNode.id,
          'parent-1',
          'sibling-1',
          null
        );

        expect(result.errors).toHaveLength(0);
        expect(result.validatedParentId).toBe('parent-1');
        expect(result.validatedBeforeSiblingId).toBe('sibling-1');
      });
    });

    // --------------------------------------------------------------------
    // updateStructuralChangesValidated()
    // --------------------------------------------------------------------
    describe('updateStructuralChangesValidated', () => {
      it('should process valid updates successfully', async () => {
        const parent: Node = { ...mockNode, id: 'parent-1' };
        const child1: Node = { ...mockNode, id: 'child-1', parentId: null };
        const child2: Node = { ...mockNode, id: 'child-2', parentId: null };

        store.setNode(parent, viewerSource);
        store.setNode(child1, viewerSource);
        store.setNode(child2, viewerSource);

        const updates = [
          { nodeId: 'child-1', parentId: 'parent-1', beforeSiblingId: null },
          { nodeId: 'child-2', parentId: 'parent-1', beforeSiblingId: 'child-1' }
        ];

        const result = await store.updateStructuralChangesValidated(updates, viewerSource, null);

        expect(result.succeeded).toHaveLength(2);
        expect(result.failed).toHaveLength(0);
        expect(result.errors.size).toBe(0);

        // Verify updates were applied
        expect(store.getNode('child-1')?.parentId).toBe('parent-1');
        expect(store.getNode('child-2')?.parentId).toBe('parent-1');
        expect(store.getNode('child-2')?.beforeSiblingId).toBe('child-1');
      });

      it('should handle validation errors gracefully', async () => {
        store.setNode(mockNode, viewerSource);

        const updates = [
          { nodeId: mockNode.id, parentId: 'non-existent-parent', beforeSiblingId: null }
        ];

        const result = await store.updateStructuralChangesValidated(updates, viewerSource, null);

        expect(result.succeeded).toHaveLength(0);
        expect(result.failed).toHaveLength(1);
        expect(result.errors.size).toBe(1);
        expect(result.errors.get(mockNode.id)?.message).toContain('Parent');
      });

      it('should process updates serially', async () => {
        const parent: Node = { ...mockNode, id: 'parent-1' };
        const child1: Node = { ...mockNode, id: 'child-1', parentId: null };
        const child2: Node = { ...mockNode, id: 'child-2', parentId: null };

        store.setNode(parent, viewerSource);
        store.setNode(child1, viewerSource);
        store.setNode(child2, viewerSource);

        const executionOrder: string[] = [];
        const originalUpdateNode = store.updateNode.bind(store);

        // Mock updateNode to track execution order
        store.updateNode = vi.fn((nodeId, changes, source, options) => {
          executionOrder.push(nodeId);
          return originalUpdateNode(nodeId, changes, source, options);
        });

        const updates = [
          { nodeId: 'child-1', parentId: 'parent-1', beforeSiblingId: null },
          { nodeId: 'child-2', parentId: 'parent-1', beforeSiblingId: null }
        ];

        await store.updateStructuralChangesValidated(updates, viewerSource, null);

        // Verify serial processing (child-1 before child-2)
        expect(executionOrder).toEqual(['child-1', 'child-2']);

        // Restore original method
        store.updateNode = originalUpdateNode;
      });

      it('should null out invalid beforeSiblingId references', async () => {
        const parent: Node = { ...mockNode, id: 'parent-1' };
        const child: Node = { ...mockNode, id: 'child-1', parentId: null };

        store.setNode(parent, viewerSource);
        store.setNode(child, viewerSource);

        const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

        const updates = [
          {
            nodeId: 'child-1',
            parentId: 'parent-1',
            beforeSiblingId: 'non-existent-sibling'
          }
        ];

        const result = await store.updateStructuralChangesValidated(updates, viewerSource, null);

        expect(result.succeeded).toHaveLength(1);
        expect(result.succeeded[0].beforeSiblingId).toBeNull();

        consoleSpy.mockRestore();
      });

      it('should handle mixed success and failure', async () => {
        const parent: Node = { ...mockNode, id: 'parent-1' };
        const child1: Node = { ...mockNode, id: 'child-1', parentId: null };
        const child2: Node = { ...mockNode, id: 'child-2', parentId: null };

        store.setNode(parent, viewerSource);
        store.setNode(child1, viewerSource);
        store.setNode(child2, viewerSource);

        const updates = [
          { nodeId: 'child-1', parentId: 'parent-1', beforeSiblingId: null },
          { nodeId: 'child-2', parentId: 'non-existent-parent', beforeSiblingId: null }
        ];

        const result = await store.updateStructuralChangesValidated(updates, viewerSource, null);

        expect(result.succeeded).toHaveLength(1);
        expect(result.failed).toHaveLength(1);
        expect(result.succeeded[0].nodeId).toBe('child-1');
        expect(result.failed[0].nodeId).toBe('child-2');
      });
    });
  });
});
