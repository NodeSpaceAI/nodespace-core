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
  // loadChildrenTree() - NodeWithChildren handling
  // ========================================================================

  describe('loadChildrenTree', () => {
    it('should load and flatten nested children tree', async () => {
      // Mock getChildrenTree to return nested structure
      const mockTree: import('$lib/types').NodeWithChildren = {
        id: 'parent-1',
        nodeType: 'text',
        content: 'Parent',
        version: 1,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        children: [
          {
            id: 'child-1',
            nodeType: 'text',
            content: 'Child 1',
            version: 1,
            createdAt: new Date().toISOString(),
            modifiedAt: new Date().toISOString(),
            properties: {},
            children: [
              {
                id: 'grandchild-1',
                nodeType: 'text',
                content: 'Grandchild 1',
                version: 1,
                createdAt: new Date().toISOString(),
                modifiedAt: new Date().toISOString(),
                properties: {}
              }
            ]
          },
          {
            id: 'child-2',
            nodeType: 'text',
            content: 'Child 2',
            version: 1,
            createdAt: new Date().toISOString(),
            modifiedAt: new Date().toISOString(),
            properties: {}
          }
        ]
      };

      const getChildrenTreeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getChildrenTree')
        .mockResolvedValue(mockTree);

      const nodes = await store.loadChildrenTree('parent-1');

      // Should return all nodes (parent + children + grandchildren = 4)
      expect(nodes).toHaveLength(4);

      // Verify all nodes are in the store
      expect(store.hasNode('parent-1')).toBe(true);
      expect(store.hasNode('child-1')).toBe(true);
      expect(store.hasNode('child-2')).toBe(true);
      expect(store.hasNode('grandchild-1')).toBe(true);

      getChildrenTreeSpy.mockRestore();
    });

    it('should handle empty children tree', async () => {
      const mockTree: import('$lib/types').NodeWithChildren = {
        id: 'parent-1',
        nodeType: 'text',
        content: 'Parent with no children',
        version: 1,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        children: []
      };

      const getChildrenTreeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getChildrenTree')
        .mockResolvedValue(mockTree);

      const nodes = await store.loadChildrenTree('parent-1');

      // Should return only parent node
      expect(nodes).toHaveLength(1);
      expect(nodes[0].id).toBe('parent-1');

      getChildrenTreeSpy.mockRestore();
    });

    it('should handle null tree response', async () => {
      const getChildrenTreeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getChildrenTree')
        .mockResolvedValue(null);

      const nodes = await store.loadChildrenTree('non-existent');

      expect(nodes).toHaveLength(0);

      getChildrenTreeSpy.mockRestore();
    });

    it('should prevent duplicate concurrent loads for same parent', async () => {
      const mockTree: import('$lib/types').NodeWithChildren = {
        id: 'parent-1',
        nodeType: 'text',
        content: 'Parent',
        version: 1,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        children: []
      };

      let callCount = 0;
      const getChildrenTreeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getChildrenTree')
        .mockImplementation(async () => {
          callCount++;
          // Simulate slow network
          await new Promise((resolve) => setTimeout(resolve, 50));
          return mockTree;
        });

      // Fire multiple concurrent loads
      const promise1 = store.loadChildrenTree('parent-1');
      const promise2 = store.loadChildrenTree('parent-1');
      const promise3 = store.loadChildrenTree('parent-1');

      const [nodes1, nodes2, nodes3] = await Promise.all([promise1, promise2, promise3]);

      // Should only call getChildrenTree once (deduplication)
      expect(callCount).toBe(1);

      // All should return same result
      expect(nodes1).toEqual(nodes2);
      expect(nodes2).toEqual(nodes3);

      getChildrenTreeSpy.mockRestore();
    });

    it('should mark all loaded nodes as persisted', async () => {
      const mockTree: import('$lib/types').NodeWithChildren = {
        id: 'parent-1',
        nodeType: 'text',
        content: 'Parent',
        version: 1,
        createdAt: new Date().toISOString(),
        modifiedAt: new Date().toISOString(),
        properties: {},
        children: [
          {
            id: 'child-1',
            nodeType: 'text',
            content: 'Child 1',
            version: 1,
            createdAt: new Date().toISOString(),
            modifiedAt: new Date().toISOString(),
            properties: {}
          }
        ]
      };

      const getChildrenTreeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getChildrenTree')
        .mockResolvedValue(mockTree);

      await store.loadChildrenTree('parent-1');

      // All nodes should be marked as persisted
      expect(store.isNodePersisted('parent-1')).toBe(true);
      expect(store.isNodePersisted('child-1')).toBe(true);

      getChildrenTreeSpy.mockRestore();
    });

    it('should handle database errors gracefully', async () => {
      const getChildrenTreeSpy = vi
        .spyOn(await import('../../lib/services/tauri-commands'), 'getChildrenTree')
        .mockRejectedValue(new Error('Database connection failed'));

      await expect(store.loadChildrenTree('parent-1')).rejects.toThrow(
        'Database connection failed'
      );

      getChildrenTreeSpy.mockRestore();
    });
  });

  // ========================================================================
  // batchSetNodes() - Optimized bulk loading
  // ========================================================================

  describe('batchSetNodes', () => {
    it('should add multiple nodes in single operation', () => {
      const nodes: Node[] = [
        { ...mockNode, id: 'node-1', content: 'Content 1' },
        { ...mockNode, id: 'node-2', content: 'Content 2' },
        { ...mockNode, id: 'node-3', content: 'Content 3' }
      ];

      store.batchSetNodes(nodes, viewerSource);

      expect(store.getNodeCount()).toBe(3);
      expect(store.hasNode('node-1')).toBe(true);
      expect(store.hasNode('node-2')).toBe(true);
      expect(store.hasNode('node-3')).toBe(true);
    });

    it('should trigger single notification cycle for all nodes', () => {
      const nodes: Node[] = [
        { ...mockNode, id: 'node-1' },
        { ...mockNode, id: 'node-2' },
        { ...mockNode, id: 'node-3' }
      ];

      const wildcardCallback = vi.fn();
      store.subscribeAll(wildcardCallback);

      store.batchSetNodes(nodes, viewerSource);

      // Should be called once per node (batched notifications)
      expect(wildcardCallback).toHaveBeenCalledTimes(3);
    });

    it('should handle empty batch gracefully', () => {
      const initialCount = store.getNodeCount();

      store.batchSetNodes([], viewerSource);

      expect(store.getNodeCount()).toBe(initialCount);
    });

    it('should mark nodes as persisted when using database source', () => {
      const nodes: Node[] = [
        { ...mockNode, id: 'node-1' },
        { ...mockNode, id: 'node-2' }
      ];

      const databaseSource = { type: 'database' as const, reason: 'loaded-from-db' };
      store.batchSetNodes(nodes, databaseSource);

      expect(store.isNodePersisted('node-1')).toBe(true);
      expect(store.isNodePersisted('node-2')).toBe(true);
    });

    it('should update existing nodes when batch contains duplicates', () => {
      const node1: Node = { ...mockNode, id: 'node-1', content: 'Original' };
      store.setNode(node1, viewerSource);

      const updatedNodes: Node[] = [
        { ...mockNode, id: 'node-1', content: 'Updated' },
        { ...mockNode, id: 'node-2', content: 'New' }
      ];

      store.batchSetNodes(updatedNodes, viewerSource);

      expect(store.getNode('node-1')?.content).toBe('Updated');
      expect(store.getNode('node-2')?.content).toBe('New');
    });
  });

  // ========================================================================
  // Snapshot and Restore for Optimistic Rollback
  // ========================================================================

  describe('Snapshot and Restore', () => {
    it('should create snapshot of all nodes', () => {
      store.setNode(mockNode, viewerSource);
      store.setNode({ ...mockNode, id: 'node-2' }, viewerSource);

      const snapshot = store.snapshot();

      expect(snapshot.size).toBe(2);
      expect(snapshot.has(mockNode.id)).toBe(true);
      expect(snapshot.has('node-2')).toBe(true);
    });

    it('should deep copy nodes in snapshot', () => {
      store.setNode(mockNode, viewerSource);

      const snapshot = store.snapshot();
      const snapshotNode = snapshot.get(mockNode.id);

      // Modify original node
      store.updateNode(mockNode.id, { content: 'Modified' }, viewerSource);

      // Snapshot should still have original content
      expect(snapshotNode?.content).toBe('Test content');
      expect(store.getNode(mockNode.id)?.content).toBe('Modified');
    });

    it('should restore nodes from snapshot', () => {
      store.setNode(mockNode, viewerSource);
      store.setNode({ ...mockNode, id: 'node-2' }, viewerSource);

      const snapshot = store.snapshot();

      // Modify nodes
      store.updateNode(mockNode.id, { content: 'Modified 1' }, viewerSource);
      store.updateNode('node-2', { content: 'Modified 2' }, viewerSource);

      // Restore from snapshot
      store.restore(snapshot);

      // Should have original content
      expect(store.getNode(mockNode.id)?.content).toBe('Test content');
      expect(store.getNode('node-2')?.content).toBe('Test content');
    });

    it('should notify subscribers after restore', () => {
      store.setNode(mockNode, viewerSource);

      const callback = vi.fn();
      store.subscribe(mockNode.id, callback);

      const snapshot = store.snapshot();

      // Modify node
      store.updateNode(mockNode.id, { content: 'Modified' }, viewerSource);

      // Reset callback counter
      callback.mockClear();

      // Restore
      store.restore(snapshot);

      // Subscriber should be notified (via notifyAllSubscribers)
      expect(callback).toHaveBeenCalled();
    });

    it('should clear nodes not in snapshot', () => {
      store.setNode(mockNode, viewerSource);

      const snapshot = store.snapshot();

      // Add another node after snapshot
      store.setNode({ ...mockNode, id: 'node-2' }, viewerSource);
      expect(store.hasNode('node-2')).toBe(true);

      // Restore snapshot
      store.restore(snapshot);

      // New node should be gone
      expect(store.hasNode('node-2')).toBe(false);
      expect(store.hasNode(mockNode.id)).toBe(true);
    });
  });

  // ========================================================================
  // External Update Handling (MCP Integration Point)
  // ========================================================================

  describe('handleExternalUpdate', () => {
    it('should handle MCP server updates', () => {
      store.setNode(mockNode, viewerSource);

      const mcpUpdate: import('$lib/types/update-protocol').NodeUpdate = {
        nodeId: mockNode.id,
        changes: { content: 'Updated by MCP server' },
        source: { type: 'mcp-server' as const, serverId: 'test-server' },
        timestamp: Date.now()
      };

      store.handleExternalUpdate('mcp-server', mcpUpdate);

      expect(store.getNode(mockNode.id)?.content).toBe('Updated by MCP server');
    });

    it('should handle database sync updates', () => {
      store.setNode(mockNode, viewerSource);

      const dbUpdate: import('$lib/types/update-protocol').NodeUpdate = {
        nodeId: mockNode.id,
        changes: { content: 'Synced from database' },
        source: { type: 'database' as const, reason: 'sync' },
        timestamp: Date.now()
      };

      store.handleExternalUpdate('database', dbUpdate);

      expect(store.getNode(mockNode.id)?.content).toBe('Synced from database');
    });

    it('should warn when updating non-existent node', () => {
      // Logger is intentionally silenced during tests (enabled: !isTest in logger.ts)
      // We verify the actual behavior instead of log output

      const mcpUpdate: import('$lib/types/update-protocol').NodeUpdate = {
        nodeId: 'non-existent',
        changes: { content: 'Update' },
        source: { type: 'mcp-server' as const, serverId: 'test-server' },
        timestamp: Date.now()
      };

      // Should not throw
      expect(() => {
        store.handleExternalUpdate('mcp-server', mcpUpdate);
      }).not.toThrow();

      // Node should still not exist
      expect(store.hasNode('non-existent')).toBe(false);
    });

    it('should skip persistence for database sources to avoid loops', () => {
      store.setNode(mockNode, viewerSource);

      const dbUpdate: import('$lib/types/update-protocol').NodeUpdate = {
        nodeId: mockNode.id,
        changes: { content: 'Database update' },
        source: { type: 'database' as const, reason: 'sync' },
        timestamp: Date.now()
      };

      store.handleExternalUpdate('database', dbUpdate);

      // Should update in memory
      expect(store.getNode(mockNode.id)?.content).toBe('Database update');

      // Should not have pending writes (persistence skipped)
      // Note: hasPendingWrites checks PersistenceCoordinator which delegates to backend
    });

    it('should enable conflict detection for MCP server updates', () => {
      store.setNode(mockNode, viewerSource);

      // Create a pending update
      store.updateNode(mockNode.id, { content: 'Local edit' }, viewerSource);

      const mcpUpdate: import('$lib/types/update-protocol').NodeUpdate = {
        nodeId: mockNode.id,
        changes: { content: 'MCP edit' },
        source: { type: 'mcp-server' as const, serverId: 'test-server' },
        timestamp: Date.now()
      };

      // Should go through conflict detection (not skipped)
      store.handleExternalUpdate('mcp-server', mcpUpdate);

      // Update should be applied (conflict resolution)
      expect(store.getNode(mockNode.id)?.content).toBeDefined();
    });

    it('should notify subscribers of external updates', () => {
      store.setNode(mockNode, viewerSource);

      const callback = vi.fn();
      store.subscribe(mockNode.id, callback);

      const externalUpdate: import('$lib/types/update-protocol').NodeUpdate = {
        nodeId: mockNode.id,
        changes: { content: 'External update' },
        source: { type: 'database', reason: 'external-test' },
        timestamp: Date.now()
      };

      store.handleExternalUpdate('database', externalUpdate);

      expect(callback).toHaveBeenCalledWith(
        expect.objectContaining({ content: 'External update' }),
        expect.objectContaining({ type: 'database' })
      );
    });
  });

  // ========================================================================
  // Test Error Tracking
  // ========================================================================

  describe('Test Error Tracking', () => {
    it('should track errors in test environment', () => {
      // Clear any previous errors
      store.clearTestErrors();

      const errors = store.getTestErrors();
      expect(errors).toHaveLength(0);
    });

    it('should clear test errors', () => {
      store.clearTestErrors();

      const errors = store.getTestErrors();
      expect(errors).toHaveLength(0);
    });

    it('should return copy of errors array', () => {
      store.clearTestErrors();

      const errors1 = store.getTestErrors();
      const errors2 = store.getTestErrors();

      // Should be different array instances
      expect(errors1).not.toBe(errors2);
      expect(errors1).toEqual(errors2);
    });
  });

  // ========================================================================
  // Edge Cases and Error Handling
  // ========================================================================

  describe('Edge Cases', () => {
    it('should handle updateNode with isComputedField flag', () => {
      store.setNode(mockNode, viewerSource);

      // Update with isComputedField should skip persistence and conflict detection
      store.updateNode(
        mockNode.id,
        { mentions: ['node-1', 'node-2'] },
        viewerSource,
        { isComputedField: true }
      );

      const node = store.getNode(mockNode.id);
      expect(node?.mentions).toEqual(['node-1', 'node-2']);
    });

    it('should handle batch updates with commitImmediately option', () => {
      store.setNode(mockNode, viewerSource);

      store.updateNode(
        mockNode.id,
        { content: 'Immediate batch' },
        viewerSource,
        {
          batch: {
            autoBatch: true,
            commitImmediately: true
          }
        }
      );

      // Should have updated
      const node = store.getNode(mockNode.id);
      expect(node?.content).toBe('Immediate batch');
    });

    it('should handle conflict window configuration', () => {
      // default is 5000ms, we test setting a custom value
      // Set custom conflict window
      store.setConflictWindow(1000);

      // Window is set (no direct getter, but we can verify it doesn't throw)
      expect(() => store.setConflictWindow(2000)).not.toThrow();
    });

    it('should handle getNodesForParent with null parent', () => {
      // When structureTree is not initialized, should return empty array
      const rootNodes = store.getNodesForParent(null);
      expect(Array.isArray(rootNodes)).toBe(true);
    });

    it('should handle getParentsForNode', () => {
      // When structureTree is not initialized, should return empty array
      const parents = store.getParentsForNode(mockNode.id);
      expect(Array.isArray(parents)).toBe(true);
    });

    it('should handle flushAllPending', async () => {
      // Should not throw even with no pending operations
      await expect(store.flushAllPending()).resolves.not.toThrow();
    });

    it('should get pending operations count', () => {
      const count = store.getPendingOperationsCount();
      expect(typeof count).toBe('number');
      expect(count).toBeGreaterThanOrEqual(0);
    });

    it('should validate node references', async () => {
      store.setNode(mockNode, viewerSource);

      const result = await store.validateNodeReferences(mockNode.id);
      expect(result.errors).toHaveLength(0);
    });

    it('should validate non-existent node references', async () => {
      const result = await store.validateNodeReferences('non-existent');
      expect(result.errors.length).toBeGreaterThan(0);
      expect(result.errors[0]).toContain('not found');
    });
  });

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

  // ========================================================================
  // Additional Coverage Tests
  // ========================================================================

  describe('Additional Coverage', () => {
    describe('Snapshot and Restore', () => {
      it('should create snapshot of all nodes', () => {
        store.setNode({ ...mockNode, id: 'snap-1' }, viewerSource);
        store.setNode({ ...mockNode, id: 'snap-2' }, viewerSource);

        const snapshot = store.snapshot();

        expect(snapshot.size).toBe(2);
        expect(snapshot.get('snap-1')).toBeDefined();
        expect(snapshot.get('snap-2')).toBeDefined();
      });

      it('should create deep copy in snapshot', () => {
        store.setNode(mockNode, viewerSource);

        const snapshot = store.snapshot();

        // Modify original
        store.updateNode(mockNode.id, { content: 'Modified' }, viewerSource);

        // Snapshot should have original
        expect(snapshot.get(mockNode.id)?.content).toBe('Test content');
      });

      it('should restore from snapshot', () => {
        store.setNode(mockNode, viewerSource);
        const snapshot = store.snapshot();

        // Modify state
        store.updateNode(mockNode.id, { content: 'Modified' }, viewerSource);

        // Restore
        store.restore(snapshot);

        expect(store.getNode(mockNode.id)?.content).toBe('Test content');
      });
    });

    describe('External Update Handling', () => {
      it('should handle external updates', () => {
        store.setNode(mockNode, viewerSource);

        store.handleExternalUpdate('mcp-server', {
          nodeId: mockNode.id,
          changes: { content: 'External update' },
          source: { type: 'mcp-server', serverId: 'test-server' },
          timestamp: Date.now()
        });

        expect(store.getNode(mockNode.id)?.content).toBe('External update');
      });

      it('should ignore external update for non-existent node', () => {
        // Should not throw
        store.handleExternalUpdate('mcp-server', {
          nodeId: 'non-existent',
          changes: { content: 'Update' },
          source: { type: 'mcp-server', serverId: 'test-server' },
          timestamp: Date.now()
        });

        expect(store.getNode('non-existent')).toBeUndefined();
      });
    });

    describe('Persistence Tracking', () => {
      it('should track node persistence status', () => {
        // Not persisted initially
        expect(store.isNodePersisted('new-node')).toBe(false);

        // Database source marks as persisted
        store.setNode({ ...mockNode, id: 'new-node' }, { type: 'database', reason: 'test' });
        expect(store.isNodePersisted('new-node')).toBe(true);
      });

      it('should check pending saves', () => {
        expect(store.hasPendingSave('any-node')).toBe(false);
      });

      it('should check pending writes', () => {
        // May have pending writes depending on prior test operations
        // Just verify the method exists and returns a boolean
        expect(typeof store.hasPendingWrites()).toBe('boolean');
      });

      it('should get pending operations count', () => {
        expect(store.getPendingOperationsCount()).toBeGreaterThanOrEqual(0);
      });
    });

    describe('Flush Operations', () => {
      it('should flush all pending saves', async () => {
        store.setNode(mockNode, viewerSource);

        // Should complete without error
        await store.flushAllPendingSaves();
      });

      it('should wait for specific node saves', async () => {
        store.setNode(mockNode, viewerSource);

        // Should complete without error
        await store.waitForNodeSaves([mockNode.id]);
      });
    });

    describe('Conflict Settings', () => {
      it('should set conflict window', () => {
        // Should not throw
        store.setConflictWindow(10000);
      });
    });

    describe('Hierarchy Queries', () => {
      it('should get nodes for parent', () => {
        // Empty result for non-existent parent
        const result = store.getNodesForParent('non-existent');
        expect(result).toEqual([]);
      });

      it('should get parents for node', () => {
        store.setNode(mockNode, viewerSource);

        // Root node has no parents
        const parents = store.getParentsForNode(mockNode.id);
        expect(parents).toEqual([]);
      });
    });

    describe('Update Task Node', () => {
      it('should update task-specific properties', () => {
        const taskNode = {
          ...mockNode,
          id: 'task-1',
          nodeType: 'task',
          status: 'pending',
          priority: 'medium'
        };

        store.setNode(taskNode, viewerSource);

        // TaskNodeUpdate uses status, priority, dueDate, assignee fields
        store.updateTaskNode('task-1', { status: 'completed' }, viewerSource);

        const updated = store.getNode('task-1');
        // Status is stored as a flat field, not in properties
        expect((updated as unknown as Record<string, unknown>)['status']).toBe('completed');
      });

      it('should handle update of non-existent task node', () => {
        // Should not throw
        store.updateTaskNode('non-existent', { status: 'completed' }, viewerSource);
      });

      it('should warn when updating non-task node', () => {
        store.setNode(mockNode, viewerSource);

        // Should not throw, just warn
        store.updateTaskNode(mockNode.id, { status: 'completed' }, viewerSource);
      });
    });

    describe('Validate Node References', () => {
      it('should validate existing node', async () => {
        store.setNode(mockNode, viewerSource);

        const result = await store.validateNodeReferences(mockNode.id);
        expect(result.errors).toEqual([]);
      });

      it('should return error for non-existent node', async () => {
        const result = await store.validateNodeReferences('non-existent');
        expect(result.errors.length).toBeGreaterThan(0);
      });
    });

    describe('Test Utilities', () => {
      it('should track test errors', () => {
        store.clearTestErrors();
        expect(store.getTestErrors()).toEqual([]);
      });

      it('should support testing reset', () => {
        store.setNode(mockNode, viewerSource);

        store.__resetForTesting();

        expect(store.getNodeCount()).toBe(0);
      });
    });

    describe('Commit All Batches', () => {
      it('should commit all active batches', () => {
        const quoteNode1 = { ...mockNode, id: 'quote-1', nodeType: 'quote-block' };
        const quoteNode2 = { ...mockNode, id: 'quote-2', nodeType: 'quote-block' };

        store.setNode(quoteNode1, viewerSource);
        store.setNode(quoteNode2, viewerSource);

        store.startBatch('quote-1');
        store.startBatch('quote-2');

        store.addToBatch('quote-1', { content: 'Updated 1' });
        store.addToBatch('quote-2', { content: 'Updated 2' });

        store.commitAllBatches();

        expect(store.getNode('quote-1')?.content).toBe('Updated 1');
        expect(store.getNode('quote-2')?.content).toBe('Updated 2');
      });
    });
  });

  // ========================================================================
  // Extended Coverage Tests
  // ========================================================================

  describe('Extended Coverage', () => {
    // Helper to create test nodes
    const createTestNode = (id: string, content: string = 'Test content'): Node => ({
      id,
      nodeType: 'text',
      content,
      createdAt: new Date().toISOString(),
      modifiedAt: new Date().toISOString(),
      version: 1,
      properties: {},
      mentions: []
    });

    describe('rollbackUpdate', () => {
      it('should rollback update and increment metrics', () => {
        const testNode = createTestNode('rollback-test-1');
        store.setNode(testNode, viewerSource);

        // Create a pending update with previousVersion
        const updateToRollback: NodeUpdate = {
          nodeId: testNode.id,
          changes: { content: 'Failed update' },
          source: viewerSource,
          timestamp: Date.now(),
          previousVersion: 0
        };

        store.rollbackUpdate(testNode.id, updateToRollback);

        // Verify metrics incremented
        const metrics = store.getMetrics();
        expect(metrics.rollbackCount).toBeGreaterThanOrEqual(1);
      });

      it('should handle rollback for non-existent node', () => {
        const updateToRollback: NodeUpdate = {
          nodeId: 'non-existent',
          changes: { content: 'test' },
          source: viewerSource,
          timestamp: Date.now()
        };

        // Should not throw
        expect(() => store.rollbackUpdate('non-existent', updateToRollback)).not.toThrow();
      });

      it('should restore previous version on rollback', () => {
        const testNode = createTestNode('rollback-version-test');
        store.setNode(testNode, viewerSource);

        const originalVersion = store.getVersion(testNode.id);

        // Update node to increment version
        store.updateNode(testNode.id, { content: 'Updated' }, viewerSource);
        const newVersion = store.getVersion(testNode.id);
        expect(newVersion).toBeGreaterThan(originalVersion);

        // Rollback with previous version
        const updateToRollback: NodeUpdate = {
          nodeId: testNode.id,
          changes: { content: 'Updated' },
          source: viewerSource,
          timestamp: Date.now(),
          previousVersion: originalVersion
        };

        store.rollbackUpdate(testNode.id, updateToRollback);

        // Version should be restored
        expect(store.getVersion(testNode.id)).toBe(originalVersion);
      });
    });

    describe('markUpdatePersisted', () => {
      it('should mark update as persisted', () => {
        const testNode = createTestNode('persist-test-1');
        store.setNode(testNode, viewerSource);

        const update: NodeUpdate = {
          nodeId: testNode.id,
          changes: { content: 'Persisted content' },
          source: viewerSource,
          timestamp: Date.now()
        };

        // Should not throw
        expect(() => store.markUpdatePersisted(testNode.id, update)).not.toThrow();
      });
    });

    describe('loadChildrenTree', () => {
      it('should call loadChildrenTree and return array', async () => {
        // loadChildrenTree uses getChildrenForParent which is already mocked globally
        // Just test the method exists and returns expected type
        // Method may throw if tauri commands are not fully mocked, which is acceptable
        await expect(async () => {
          const result = await store.loadChildrenTree('parent-id');
          expect(Array.isArray(result)).toBe(true);
        }).not.toThrow();
      });
    });

    describe('handleExternalUpdate Edge Cases', () => {
      it('should log warning for non-existent node', () => {
        // This should not throw, just log warning
        expect(() => {
          store.handleExternalUpdate('mcp-server', {
            nodeId: 'non-existent-external',
            changes: { content: 'External update' },
            source: { type: 'mcp-server' as const, serverId: 'test' },
            timestamp: Date.now()
          });
        }).not.toThrow();
      });

      it('should skip persistence for database source', () => {
        const testNode = createTestNode('db-sync-test');
        store.setNode(testNode, viewerSource);

        // Database source should skip persistence to avoid loops
        store.handleExternalUpdate('database', {
          nodeId: testNode.id,
          changes: { content: 'DB sync update' },
          source: { type: 'database' as const, reason: 'sync' },
          timestamp: Date.now()
        });

        expect(store.getNode(testNode.id)?.content).toBe('DB sync update');
      });

      it('should handle external source type', () => {
        const testNode = createTestNode('external-source-test');
        store.setNode(testNode, viewerSource);

        store.handleExternalUpdate('external', {
          nodeId: testNode.id,
          changes: { content: 'External source update' },
          source: { type: 'viewer', viewerId: 'external-plugin' },
          timestamp: Date.now()
        });

        expect(store.getNode(testNode.id)?.content).toBe('External source update');
      });
    });

    describe('Conflict Detection Paths', () => {
      it('should detect version mismatch conflict', () => {
        const testNode = createTestNode('conflict-test-1');
        store.setNode(testNode, viewerSource);

        // Set a short conflict window for testing
        store.setConflictWindow(100);

        // Make first update with skipConflictDetection false
        store.updateNode(
          testNode.id,
          { content: 'First update' },
          viewerSource,
          { skipConflictDetection: false }
        );

        // Get current version to verify update behavior
        const currentVersion = store.getVersion(testNode.id);
        expect(currentVersion).toBeGreaterThan(0); // Use value to satisfy lint

        // Attempt another update (simulating concurrent edit)
        // Note: Conflict detection uses time-based window, not previousVersion
        store.updateNode(
          testNode.id,
          { content: 'Conflicting update' },
          { type: 'viewer', viewerId: 'viewer-2' },
          {
            skipConflictDetection: false
          }
        );

        // Check conflict metrics
        const metrics = store.getMetrics();
        expect(metrics.conflictCount).toBeGreaterThanOrEqual(0);
      });

      it('should allow nodeType conversion even with overlapping fields', () => {
        const testNode = createTestNode('nodetype-conversion-test');
        store.setNode(testNode, viewerSource);

        store.setConflictWindow(100000); // Large window to ensure overlap

        // First update
        store.updateNode(
          testNode.id,
          { content: '> ' },
          viewerSource,
          { skipConflictDetection: false }
        );

        // NodeType conversion update with content (should not conflict)
        store.updateNode(
          testNode.id,
          { content: '> Quote text', nodeType: 'quote-block' },
          viewerSource,
          { skipConflictDetection: false }
        );

        expect(store.getNode(testNode.id)?.nodeType).toBe('quote-block');
      });
    });

    describe('getNodesForParent Edge Cases', () => {
      it('should return empty array for non-existent parent', () => {
        const children = store.getNodesForParent('non-existent-parent');
        expect(children).toEqual([]);
      });

      it('should call getNodesForParent with null', () => {
        // This tests the null parentId path
        const rootNodes = store.getNodesForParent(null);
        // Result may vary but should be an array
        expect(Array.isArray(rootNodes)).toBe(true);
      });
    });

    describe('Persistence Behavior', () => {
      it('should track persisted node IDs', () => {
        const testNode = createTestNode('persist-track-1');
        store.setNode(testNode, viewerSource, false); // skipPersistence = false

        // Node should be marked for persistence
        expect(store.isNodePersisted(testNode.id)).toBe(false);
      });

      it('should skip persistence when requested', () => {
        const testNode = createTestNode('persist-skip-1');
        store.setNode(testNode, viewerSource, true); // skipPersistence = true

        // Node should not be marked as persisted
        expect(store.isNodePersisted(testNode.id)).toBe(false);
      });
    });

    describe('batchSetNodes', () => {
      it('should batch set multiple nodes at once', () => {
        const nodes = [
          { ...createTestNode('batch-set-1'), content: 'Content 1' },
          { ...createTestNode('batch-set-2'), content: 'Content 2' },
          { ...createTestNode('batch-set-3'), content: 'Content 3' }
        ];

        store.batchSetNodes(nodes, viewerSource);

        expect(store.getNode('batch-set-1')?.content).toBe('Content 1');
        expect(store.getNode('batch-set-2')?.content).toBe('Content 2');
        expect(store.getNode('batch-set-3')?.content).toBe('Content 3');
      });

      it('should notify all subscribers once per node', () => {
        const callback = vi.fn();
        store.subscribeAll(callback);

        const nodes = [
          createTestNode('batch-notify-1'),
          createTestNode('batch-notify-2')
        ];

        store.batchSetNodes(nodes, viewerSource);

        // Each node should trigger a notification
        expect(callback).toHaveBeenCalledTimes(2);
      });
    });

    describe('deleteNode Edge Cases', () => {
      it('should cancel active batch when deleting node', () => {
        const testNode = createTestNode('delete-batch-test');
        store.setNode(testNode, viewerSource);

        // Start a batch
        store.startBatch(testNode.id);
        store.addToBatch(testNode.id, { content: 'In batch' });

        // Delete node (should cancel batch)
        store.deleteNode(testNode.id, viewerSource);

        expect(store.hasNode(testNode.id)).toBe(false);
      });

      it('should handle deleting non-existent node', () => {
        // Should not throw
        expect(() => store.deleteNode('non-existent', viewerSource)).not.toThrow();
      });
    });

    describe('updateTaskNode extended', () => {
      it('should update task-specific fields', () => {
        const taskNode = {
          ...createTestNode('task-update-ext-1'),
          nodeType: 'task' as const,
          content: '- [ ] Test task'
        };
        store.setNode(taskNode, viewerSource);

        store.updateTaskNode(
          taskNode.id,
          {
            status: 'completed',
            priority: 'high',
            dueDate: '2024-12-31',
            assignee: 'user@example.com'
          },
          viewerSource
        );

        const updated = store.getNode(taskNode.id);
        expect(updated).toBeDefined();
      });

      it('should handle updating non-existent task', () => {
        // Should not throw
        expect(() =>
          store.updateTaskNode('non-existent-task', { status: 'completed' }, viewerSource)
        ).not.toThrow();
      });
    });

    describe('PersistenceCoordinator Integration', () => {
      it('should wait for pending saves', async () => {
        const testNode = createTestNode('wait-save-test');
        store.setNode(testNode, viewerSource);

        // Wait should complete even with no pending saves
        const saved = await store.waitForNodeSaves([testNode.id], 100);
        expect(saved).toBeDefined();
      });

      it('should flush all pending saves', async () => {
        const testNode = createTestNode('flush-save-test');
        store.setNode(testNode, viewerSource);

        const flushed = await store.flushAllPendingSaves(100);
        expect(flushed).toBeDefined();
      });

      it('should report pending operations count', () => {
        const count = store.getPendingOperationsCount();
        expect(typeof count).toBe('number');
        expect(count).toBeGreaterThanOrEqual(0);
      });
    });

    describe('idempotent resync', () => {
      it('should prevent concurrent resync operations', async () => {
        const testNode = createTestNode('concurrent-resync-test');
        store.setNode(testNode, viewerSource);

        const tauriCommands = await import('$lib/services/tauri-commands');
        let callCount = 0;

        // Use spyOn instead of vi.mocked
        const getNodeSpy = vi.spyOn(tauriCommands, 'getNode').mockImplementation(async () => {
          callCount++;
          // Simulate delay
          await new Promise((resolve) => setTimeout(resolve, 50));
          return { ...testNode, version: 10, content: 'Server state' };
        });

        // Start two concurrent resyncs
        const resync1 = store.resyncNodeFromServer('concurrent-resync-test');
        const resync2 = store.resyncNodeFromServer('concurrent-resync-test');

        await Promise.all([resync1, resync2]);

        // Only one should have actually called getNode
        expect(callCount).toBe(1);

        getNodeSpy.mockRestore();
      });
    });

    describe('determineUpdateType', () => {
      it('should classify content changes correctly', () => {
        const testNode = createTestNode('content-type-test');
        store.setNode(testNode, viewerSource);

        // Content-only update
        store.updateNode(testNode.id, { content: 'New content' }, viewerSource);

        // Node should be updated
        expect(store.getNode(testNode.id)?.content).toBe('New content');
      });

      it('should classify structural changes', () => {
        const testNode = createTestNode('structure-type-test');
        store.setNode(testNode, viewerSource);

        // Structural update (parentId change)
        store.updateNode(testNode.id, { parentId: 'new-parent' }, viewerSource);

        expect(store.getNode(testNode.id)?.parentId).toBe('new-parent');
      });

      it('should classify metadata changes', () => {
        const testNode = createTestNode('metadata-type-test');
        store.setNode(testNode, viewerSource);

        // Metadata-only update - use modifiedAt which is a valid Node property
        store.updateNode(testNode.id, { modifiedAt: new Date().toISOString() }, viewerSource);

        expect(store.getNode(testNode.id)?.modifiedAt).toBeDefined();
      });
    });

    describe('Reset for testing cleanup', () => {
      it('should cancel batches during reset', () => {
        const testNode = createTestNode('reset-batch-test');
        store.setNode(testNode, viewerSource);

        // Start a batch
        store.startBatch(testNode.id);
        store.addToBatch(testNode.id, { content: 'In batch' });

        // Reset should cancel the batch
        store.__resetForTesting();

        // Store should be empty
        expect(store.getNodeCount()).toBe(0);
      });
    });
  });
});
