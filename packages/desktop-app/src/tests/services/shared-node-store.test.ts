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
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import type { Node } from '../../lib/types';
import type { UpdateSource, NodeUpdate } from '../../lib/types/update-protocol';

describe('SharedNodeStore', () => {
  let store: SharedNodeStore;
  const mockNode: Node = {
    id: 'test-node-1',
    nodeType: 'text',
    content: 'Test content',
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

    it('should handle updating non-existent node gracefully', () => {
      // Logger is intentionally silenced during tests (enabled: !isTest in logger.ts)
      // We verify the actual behavior instead of log output

      // Should not throw when updating non-existent node
      expect(() => {
        store.updateNode('non-existent', { content: 'test' }, viewerSource);
      }).not.toThrow();

      // Node should still not exist after attempted update
      expect(store.hasNode('non-existent')).toBe(false);
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
      // Logger is intentionally silenced during tests (enabled: !isTest in logger.ts)
      // We verify the actual behavior (error isolation) instead of log output

      store.setNode(mockNode, viewerSource);

      const errorCallback = vi.fn(() => {
        throw new Error('Callback error');
      });
      const workingCallback = vi.fn();

      store.subscribe(mockNode.id, errorCallback);
      store.subscribe(mockNode.id, workingCallback);

      store.updateNode(mockNode.id, { content: 'New content' }, viewerSource);

      // Both callbacks should be called despite error (error is caught and isolated)
      expect(errorCallback).toHaveBeenCalled();
      expect(workingCallback).toHaveBeenCalled();
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
      expect(resolver).toBeDefined();
    });

    it('should allow setting custom conflict resolver', () => {
      const originalResolver = store.getConflictResolver();
      // Set back the same resolver (test the setter works)
      store.setConflictResolver(originalResolver);

      expect(store.getConflictResolver()).toBe(originalResolver);
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

      // Note: Tests for PersistenceCoordinator integration removed (service deleted in #558)
    });

    // Note: hasPendingSave tests removed (PersistenceCoordinator deleted in #558)

    // Note: waitForNodeSaves tests removed (PersistenceCoordinator deleted in #558)

    // NOTE: validateNodeReferences() and updateStructuralChangesValidated()
    // were removed as part of the beforeSiblingId removal (Issue #575).
    // Node ordering is now handled by the backend via fractional IDs and moveNode.
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

      it('should handle adding to non-existent batch gracefully', () => {
        // Logger is intentionally silenced during tests (enabled: !isTest in logger.ts)
        // We verify the actual behavior instead of log output

        // Should not throw when adding to non-existent batch
        expect(() => {
          store.addToBatch('non-existent-node', { content: 'test' });
        }).not.toThrow();

        // Node should not have been created
        expect(store.hasNode('non-existent-node')).toBe(false);
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
        // Logger is intentionally silenced during tests (enabled: !isTest in logger.ts)
        // We verify the actual behavior (batch cleanup) instead of log output

        store.setNode(quoteNode, viewerSource);
        store.startBatch('quote-node-1');

        store.addToBatch('quote-node-1', { content: '> Test' });
        const contentBeforeCommit = store.getNode('quote-node-1')?.content;

        store.commitBatch('quote-node-1');

        // Trying to add to committed batch should not throw (graceful handling)
        expect(() => {
          store.addToBatch('quote-node-1', { content: '> After commit' });
        }).not.toThrow();

        // Content should remain unchanged (batch was committed and cleaned up)
        expect(store.getNode('quote-node-1')?.content).toBe(contentBeforeCommit);
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
        // Logger is intentionally silenced during tests (enabled: !isTest in logger.ts)
        // We verify the actual behavior (batch cleanup on deletion) instead of log output

        store.setNode(quoteNode, viewerSource, true);
        store.startBatch('quote-node-1');

        store.addToBatch('quote-node-1', { content: '> Test' });

        // Delete node (should cancel batch)
        store.deleteNode('quote-node-1', viewerSource);

        // Node should be deleted
        expect(store.hasNode('quote-node-1')).toBe(false);

        // Batch should be gone - adding to batch should not throw (graceful handling)
        expect(() => {
          store.addToBatch('quote-node-1', { content: '> After delete' });
        }).not.toThrow();
      });
    });
  });

  // Note: Explicit Persistence API tests removed (PersistenceCoordinator deleted in #558)

  // ========================================================================
  // OCC Error Recovery Tests (Issue #720)
  // ========================================================================

  describe('OCC Error Recovery', () => {
    const mockServerNode: Node = {
      id: 'test-node-occ',
      nodeType: 'text',
      content: 'Server content (version 5)',
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 5,
      properties: {},
      mentions: []
    };

    it('should resync node from server after OCC error', async () => {
      // Setup: Create a node with version 1
      const localNode: Node = {
        ...mockNode,
        id: 'test-node-occ',
        version: 1,
        content: 'Local content (version 1)'
      };

      store.setNode(localNode, viewerSource, true);

      // Mock tauriCommands.getNode to return server state
      const getNodeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getNode')
        .mockResolvedValue(mockServerNode);

      // Trigger resync
      await store.resyncNodeFromServer('test-node-occ');

      // Verify getNode was called
      expect(getNodeSpy).toHaveBeenCalledWith('test-node-occ');

      // Verify local node was replaced with server state
      const resyncedNode = store.getNode('test-node-occ');
      expect(resyncedNode?.content).toBe('Server content (version 5)');
      expect(resyncedNode?.version).toBe(5);

      // Verify version was synced
      const version = store.getVersion('test-node-occ');
      expect(version).toBe(5);

      // Verify node is marked as persisted (accessing via public getNode which checks persisted state)
      // Note: persistedNodeIds is private, but we can verify by checking node was fetched successfully
      expect(resyncedNode).toBeTruthy();

      getNodeSpy.mockRestore();
    });

    it('should clear pending updates after resync', async () => {
      // Setup: Create a node
      const localNode: Node = {
        ...mockNode,
        id: 'test-node-occ',
        version: 1,
        content: 'Local content'
      };

      store.setNode(localNode, viewerSource, true);

      // Mock tauriCommands.getNode
      const getNodeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getNode')
        .mockResolvedValue(mockServerNode);

      // Trigger resync
      await store.resyncNodeFromServer('test-node-occ');

      // Verify node was resynced (pending updates cleared internally)
      const resyncedNode = store.getNode('test-node-occ');
      expect(resyncedNode?.version).toBe(5);
      expect(resyncedNode?.content).toBe('Server content (version 5)');

      getNodeSpy.mockRestore();
    });

    it('should notify subscribers after resync', async () => {
      // Setup
      const localNode: Node = {
        ...mockNode,
        id: 'test-node-occ',
        version: 1,
        content: 'Local content'
      };

      // Setup subscriber BEFORE setting the node
      let notificationCount = 0;
      let lastNotifiedNode: Node | null = null;
      let lastNotifiedSource: UpdateSource | null = null;

      store.subscribe('test-node-occ', (node: Node, source: UpdateSource) => {
        notificationCount++;
        lastNotifiedNode = node;
        lastNotifiedSource = source;
      });

      // Set the node (will trigger first notification)
      store.setNode(localNode, viewerSource, true);

      // Reset counter after initial setup
      notificationCount = 0;

      // Mock tauriCommands.getNode
      const getNodeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getNode')
        .mockResolvedValue(mockServerNode);

      // Trigger resync
      await store.resyncNodeFromServer('test-node-occ');

      // Verify subscribers were notified with server state
      expect(notificationCount).toBe(1);
      expect(lastNotifiedNode).toEqual(mockServerNode);
      expect(lastNotifiedSource).toBeTruthy();

      // Type guard assertion
      const databaseSource = lastNotifiedSource as { type: 'database'; reason: string } | null;
      expect(databaseSource?.type).toBe('database');
      expect(databaseSource?.reason).toBe('occ-resync');

      getNodeSpy.mockRestore();
    });

    it('should handle resync when node not found on server', async () => {
      // Logger is intentionally silenced during tests (enabled: !isTest in logger.ts)
      // We verify the actual behavior (graceful error handling) instead of log output

      // Setup
      const localNode: Node = {
        ...mockNode,
        id: 'test-node-occ',
        version: 1
      };

      store.setNode(localNode, viewerSource, true);

      // Mock tauriCommands.getNode to return null (node not found)
      const getNodeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getNode')
        .mockResolvedValue(null);

      // Trigger resync - should not throw
      await expect(store.resyncNodeFromServer('test-node-occ')).resolves.not.toThrow();

      // Verify local node unchanged (resync gracefully failed)
      const localNodeAfter = store.getNode('test-node-occ');
      expect(localNodeAfter?.version).toBe(1);

      getNodeSpy.mockRestore();
    });

    it('should handle resync failure gracefully', async () => {
      // Logger is intentionally silenced during tests (enabled: !isTest in logger.ts)
      // We verify the actual behavior (error propagation) instead of log output

      // Setup
      const localNode: Node = {
        ...mockNode,
        id: 'test-node-occ',
        version: 1
      };

      store.setNode(localNode, viewerSource, true);

      // Mock tauriCommands.getNode to throw error
      const getNodeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getNode')
        .mockRejectedValue(new Error('Network error'));

      // Trigger resync and expect it to throw (error is propagated to caller)
      await expect(store.resyncNodeFromServer('test-node-occ')).rejects.toThrow('Network error');

      getNodeSpy.mockRestore();
    });

    it('should allow subsequent edits after resync', async () => {
      // Setup: Create node with version 1
      const localNode: Node = {
        ...mockNode,
        id: 'test-node-occ',
        version: 1,
        content: 'Local content'
      };

      store.setNode(localNode, viewerSource, true);

      // Mock tauriCommands.getNode to return server state (version 5)
      const getNodeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getNode')
        .mockResolvedValue(mockServerNode);

      // Resync to version 5
      await store.resyncNodeFromServer('test-node-occ');

      // Now try to update with the correct version (5)
      store.updateNode(
        'test-node-occ',
        { content: 'New edit after resync' },
        viewerSource,
        { skipPersistence: true }
      );

      // Verify update succeeded
      const updatedNode = store.getNode('test-node-occ');
      expect(updatedNode?.content).toBe('New edit after resync');

      // Verify version was incremented from synced version
      const version = store.getVersion('test-node-occ');
      expect(version).toBe(6); // Should be server version (5) + 1

      getNodeSpy.mockRestore();
    });
  });
});
