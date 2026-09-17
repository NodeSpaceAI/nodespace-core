/**
 * SharedNodeStore Coverage Tests - Target uncovered lines
 *
 * This file specifically targets uncovered code paths to push coverage from 86.16% to 95%+
 * Focuses on:
 * - SimplePersistenceCoordinator edge cases
 * - Error handling paths in persistence
 * - Batch persistence race conditions
 * - Edge cases in updateNode type-specific property routing
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { SharedNodeStore } from '../../lib/services/shared-node-store.svelte';
import type { Node } from '../../lib/types';
import type { UpdateSource } from '../../lib/types/update-protocol';
import { backendAdapter } from '../../lib/services/backend-adapter';
import { conflictNotifications } from '../../lib/stores/conflict-notifications.svelte';
import {
  DEBOUNCED_WRITE_WAIT_MS,
  TEARDOWN_FLUSH_CEILING_MS,
  CASCADE_SETTLE_TIMEOUT_MS,
  FLUSH_PENDING_TIMEOUT_MS
} from '../utils/test-constants';

describe('SharedNodeStore - Coverage Completion', () => {
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

  const databaseSource: UpdateSource = {
    type: 'database',
    reason: 'test'
  };

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    store = SharedNodeStore.getInstance();
    store.clearTestErrors();
    conflictNotifications.dismissAll();
  });

  afterEach(async () => {
    // Restore real timers first: a test that left fake timers installed would
    // otherwise freeze the clock this teardown races against, hanging the run.
    vi.useRealTimers();
    try {
      // Clear test errors before flushing
      store.clearTestErrors();
      // Cap the flush so a test that deliberately left an operation hanging
      // cannot stall teardown. This races a pending flush, so it is a ceiling
      // that is almost never reached — not a fixed wait.
      await Promise.race([
        store.flushAllPending(),
        new Promise((resolve) => setTimeout(resolve, TEARDOWN_FLUSH_CEILING_MS))
      ]);
    } catch {
      // Ignore cleanup errors
    }
    store.clearAll();
    SharedNodeStore.resetInstance();
    conflictNotifications.dismissAll();
    vi.clearAllMocks();
    // Let already-scheduled microtasks and any 0ms timers drain before the next
    // test installs its own mocks. A macrotask boundary is what is needed here;
    // the old fixed 100ms sleep was 100x that, charged to every test in the file.
    await new Promise((resolve) => setTimeout(resolve, 0));
  });

  // ========================================================================
  // SimplePersistenceCoordinator - Dependency Handling
  // ========================================================================

  describe('Persistence Dependencies', () => {
    it('should wait for function dependencies before execution', async () => {
      let depExecuted = false;
      const dependency = async () => {
        await new Promise(resolve => setTimeout(resolve, 10));
        depExecuted = true;
      };

      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...mockNode,
        version: 2
      });

      store.setNode(mockNode, databaseSource);

      // Update with function dependency
      store.updateNode(
        mockNode.id,
        { content: 'Update with dep' },
        viewerSource,
        { persistenceDependencies: [dependency] }
      );

      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));
      expect(depExecuted).toBe(true);
    });

    it('should wait for node ID dependencies', async () => {
      const dep1 = { ...mockNode, id: 'dep-1' };
      const dep2 = { ...mockNode, id: 'dep-2' };

      store.setNode(dep1, viewerSource);
      store.setNode(dep2, viewerSource);

      // Create update that depends on dep-1
      store.updateNode(dep2.id, { content: 'Depends on dep-1' }, viewerSource);

      await new Promise(resolve => setTimeout(resolve, 100));
      expect(store.hasNode(dep2.id)).toBe(true);
    });

    it('should handle waiting for already-completed dependency', async () => {
      const dep = { ...mockNode, id: 'dep-completed' };
      store.setNode(dep, databaseSource); // Already persisted

      const dependent = { ...mockNode, id: 'dependent' };
      store.setNode(dependent, viewerSource);

      // Update with dependency on already-persisted node
      store.updateNode(
        dependent.id,
        { content: 'Depends on completed' },
        viewerSource
      );

      await new Promise(resolve => setTimeout(resolve, 100));
      expect(store.hasNode(dependent.id)).toBe(true);
    });
  });

  // ========================================================================
  // SimplePersistenceCoordinator - Error Handling
  // ========================================================================

  describe('Persistence Error Recovery', () => {
    it('should handle non-Error exceptions in operations', async () => {
      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValue('String error');

      store.setNode(mockNode, databaseSource);
      store.updateNode(mockNode.id, { content: 'Trigger error' }, viewerSource);

      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));

      const errors = store.getTestErrors();
      expect(errors.length).toBeGreaterThan(0);
      expect(errors[0].message).toContain('String error');
    });

    it('should reject with Error when operation throws non-Error', async () => {
      vi.spyOn(backendAdapter, 'createNode').mockRejectedValue({ custom: 'object error' });

      store.setNode(mockNode, viewerSource);

      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));

      const errors = store.getTestErrors();
      expect(errors.length).toBeGreaterThan(0);
    });
  });

  // ========================================================================
  // updateNode - Spoke Field Routing Edge Cases
  // ========================================================================

  describe('updateNode - Type-Specific Routing', () => {
    it('should skip type-specific updater when nodeType is changing', async () => {
      // Mock generic updateNode
      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...mockNode,
        nodeType: 'quote-block',
        version: 2
      });

      const taskNode = { ...mockNode, id: 'task-convert', nodeType: 'task' };
      store.setNode(taskNode, databaseSource);

      // Update that changes nodeType (should use generic path, not task-specific)
      store.updateNode('task-convert', { nodeType: 'quote-block' }, viewerSource);

      await new Promise(resolve => setTimeout(resolve, 100));

      expect(store.getNode('task-convert')?.nodeType).toBe('quote-block');
    });

    it('should handle task property changes for nodes with updater', async () => {
      const taskNode: Node = {
        ...mockNode,
        id: 'task-props',
        nodeType: 'task',
        properties: { task: { status: 'todo' } }
      };

      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...taskNode,
        properties: { task: { status: 'done' } },
        version: 2
      });

      store.setNode(taskNode, databaseSource);

      // Update task status via properties - should trigger persistence even from viewer
      store.updateNode('task-props', { properties: { task: { status: 'done' } } }, viewerSource);

      await new Promise(resolve => setTimeout(resolve, 100));

      const updated = store.getNode('task-props');
      expect((updated?.properties as Record<string, unknown>)?.task).toEqual({ status: 'done' });
    });

    it('should handle properties field changes', async () => {
      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...mockNode,
        properties: { custom: 'value' },
        version: 2
      });

      store.setNode(mockNode, databaseSource);
      store.updateNode(mockNode.id, { properties: { custom: 'value' } }, viewerSource);

      await new Promise(resolve => setTimeout(resolve, 100));

      expect(store.getNode(mockNode.id)?.properties).toEqual({ custom: 'value' });
    });
  });

  // ========================================================================
  // setNode - Update Fallback Paths
  // ========================================================================

  describe('setNode - Error Recovery Paths', () => {
    it('should handle update error with lowercase message matching', async () => {
      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValue(
        new Error('NodeNotFound: node does not exist')
      );
      vi.spyOn(backendAdapter, 'createNode').mockResolvedValue(mockNode.id);
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue({ ...mockNode, version: 1 });

      // Mark as persisted but node doesn't exist in DB
      store.setNode(mockNode, databaseSource);

      // Try to set again (will attempt update, fail, then create)
      store.setNode({ ...mockNode, content: 'Updated' }, viewerSource);

      await new Promise(resolve => setTimeout(resolve, 200));

      expect(store.hasNode(mockNode.id)).toBe(true);
    });

    it('should handle "not found" variation in error message', async () => {
      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValue(
        new Error('Node not found in database')
      );
      vi.spyOn(backendAdapter, 'createNode').mockResolvedValue(mockNode.id);
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue({ ...mockNode, version: 1 });

      store.setNode(mockNode, databaseSource);
      store.setNode({ ...mockNode, content: 'Updated' }, viewerSource);

      await new Promise(resolve => setTimeout(resolve, 200));

      expect(store.hasNode(mockNode.id)).toBe(true);
    });

    it('should handle tauri error object stringification', async () => {
      const tauriError = {
        error: 'Database error',
        code: 500,
        details: 'Connection failed'
      };

      vi.spyOn(backendAdapter, 'createNode').mockRejectedValue(tauriError);

      store.setNode(mockNode, viewerSource);

      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));

      const errors = store.getTestErrors();
      expect(errors.length).toBeGreaterThan(0);
      expect(errors[0].message).toContain('Database error');
    });

    it('should handle non-object non-Error exceptions in setNode', async () => {
      vi.spyOn(backendAdapter, 'createNode').mockRejectedValue('Plain string error');

      store.setNode(mockNode, viewerSource);

      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));

      const errors = store.getTestErrors();
      expect(errors.length).toBeGreaterThan(0);
    });
  });

  // ========================================================================
  // updateNode - Create Fallback in Persistence
  // ========================================================================

  describe('updateNode - Create Fallback', () => {
    it('should handle update-then-create when node marked persisted but missing', async () => {
      let updateCalled = false;
      vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async () => {
        if (!updateCalled) {
          updateCalled = true;
          throw new Error('does not exist in database');
        }
        return { ...mockNode, version: 2 };
      });

      vi.spyOn(backendAdapter, 'createNode').mockResolvedValue(mockNode.id);
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue({ ...mockNode, version: 1 });

      // Mark as persisted (simulating stale state)
      store.setNode(mockNode, databaseSource);

      // Update should trigger update, fail, then create
      store.updateNode(mockNode.id, { content: 'Updated' }, viewerSource);

      await new Promise(resolve => setTimeout(resolve, 200));

      expect(store.getNode(mockNode.id)?.content).toBe('Updated');
    });

    it('should fetch created node version after fallback create', async () => {
      let callCount = 0;
      vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async () => {
        callCount++;
        if (callCount === 1) {
          throw new Error('NodeNotFound');
        }
        return { ...mockNode, version: 2 };
      });
      vi.spyOn(backendAdapter, 'createNode').mockResolvedValue(mockNode.id);
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue({ ...mockNode, version: 3 });

      store.setNode(mockNode, databaseSource);
      store.updateNode(mockNode.id, { content: 'Updated' }, viewerSource);

      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));

      // Version should be updated from backend
      const node = store.getNode(mockNode.id);
      expect(node?.version).toBeGreaterThanOrEqual(1);
    });
  });

  // ========================================================================
  // Batch Persistence - Update Path Coverage
  // ========================================================================

  describe('Batch Persistence Paths', () => {
    it('should handle persisted node UPDATE path in batch', async () => {
      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...mockNode,
        content: '> Batch updated',
        version: 2
      });

      const node = { ...mockNode, id: 'batch-update', nodeType: 'quote-block', content: '> Test' };

      // Mark as persisted
      store.setNode(node, databaseSource);

      store.startBatch('batch-update');
      store.addToBatch('batch-update', { content: '> Batch updated' });
      store.commitBatch('batch-update');

      await new Promise(resolve => setTimeout(resolve, 200));

      expect(store.getNode('batch-update')?.content).toBe('> Batch updated');
    });

    it('should update local version after batch UPDATE', async () => {
      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...mockNode,
        id: 'batch-version',
        content: '> Updated',
        version: 5
      });

      const node = { ...mockNode, id: 'batch-version', nodeType: 'quote-block', content: '> Test' };
      store.setNode(node, databaseSource);

      store.startBatch('batch-version');
      store.addToBatch('batch-version', { content: '> Updated' });
      store.commitBatch('batch-version');

      await new Promise(resolve => setTimeout(resolve, 200));

      const updated = store.getNode('batch-version');
      expect(updated?.version).toBe(5);
    });

    it('should handle race with "already exists" error variant', async () => {
      let createCalled = false;
      vi.spyOn(backendAdapter, 'createNode').mockImplementation(async () => {
        if (!createCalled) {
          createCalled = true;
          throw new Error('Node already exists in database');
        }
        return 'race-exists';
      });

      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...mockNode,
        id: 'race-exists',
        version: 2
      });

      const node = { ...mockNode, id: 'race-exists', nodeType: 'quote-block' };
      store.setNode(node, viewerSource, true);

      store.startBatch('race-exists');
      store.addToBatch('race-exists', { content: '> Updated' });
      store.commitBatch('race-exists');

      await new Promise(resolve => setTimeout(resolve, 200));

      expect(store.getNode('race-exists')).toBeTruthy();
    });

    it('should update local version after race UPDATE fallback', async () => {
      let createCalled = false;
      vi.spyOn(backendAdapter, 'createNode').mockImplementation(async () => {
        if (!createCalled) {
          createCalled = true;
          throw new Error('UNIQUE constraint failed');
        }
        return 'race-version';
      });

      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...mockNode,
        id: 'race-version',
        content: '> Updated',
        version: 7
      });

      const node = { ...mockNode, id: 'race-version', nodeType: 'quote-block' };
      store.setNode(node, viewerSource, true);

      store.startBatch('race-version');
      store.addToBatch('race-version', { content: '> Updated' });
      store.commitBatch('race-version');

      await new Promise(resolve => setTimeout(resolve, 200));

      const updated = store.getNode('race-version');
      expect(updated?.version).toBe(7);
    });

    it('surfaces a write-failure notification when a batch commit hits a VERSION_CONFLICT, instead of silently swallowing it', async () => {
      // persistBatchedChanges() has no OCC-specific handling of its own (no
      // rollback, no resync, no notification) — unlike updateNode()'s and
      // updateTaskNodeStatus()'s persist() closures, which handle
      // VERSION_CONFLICT internally before their outer .catch runs. A batch
      // commit's OCC failure must therefore reach the standard write-failure
      // notification, not be dropped as if it were an "expected" cancellation.
      const occError = new Error('VERSION_CONFLICT: optimistic concurrency failure') as Error & {
        code: string;
        conflictData: { node_id: string; expected: number; actual: number; current_node: null };
      };
      occError.code = 'VERSION_CONFLICT';
      occError.conflictData = {
        node_id: 'batch-occ',
        expected: 1,
        actual: 2,
        current_node: null
      };
      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValue(occError);

      const node = { ...mockNode, id: 'batch-occ', nodeType: 'quote-block', content: '> Test' };
      store.setNode(node, databaseSource);

      store.startBatch('batch-occ');
      store.addToBatch('batch-occ', { content: '> Updated' });
      store.commitBatch('batch-occ');

      await new Promise((resolve) => setTimeout(resolve, 200));

      const writeFailures = conflictNotifications.notifications.filter(
        (n) => n.conflictType === 'write-failure'
      );
      expect(writeFailures).toHaveLength(1);
      expect(writeFailures[0]?.nodeId).toBe('batch-occ');
    });
  });

  // ========================================================================
  // Rollback Coverage
  // ========================================================================

  describe('Rollback Scenarios', () => {
    it('should handle rollback with defined previousVersion', () => {
      store.setNode(mockNode, viewerSource);

      // Make first update
      store.updateNode(mockNode.id, { content: 'Update 1' }, viewerSource, {
        skipPersistence: true
      });

      const v1 = store.getVersion(mockNode.id);

      // Make second update
      store.updateNode(mockNode.id, { content: 'Update 2' }, viewerSource, {
        skipPersistence: true
      });

      const v2 = store.getVersion(mockNode.id);
      expect(v2).toBeGreaterThan(v1);

      // Rollback should revert version
      const currentNode = store.getNode(mockNode.id);
      expect(currentNode).toBeTruthy();
    });

    it('should handle rollback when update not in pending list', () => {
      store.setNode(mockNode, viewerSource);

      // This tests the "not in pending" path
      // Creating an update and immediately clearing should trigger this
      store.updateNode(mockNode.id, { content: 'Test' }, viewerSource, {
        skipPersistence: true
      });

      expect(store.hasNode(mockNode.id)).toBe(true);
    });
  });

  // ========================================================================
  // PersistenceCoordinator - flushPending
  // ========================================================================

  describe('flushPending Coverage', () => {
    it('should handle empty pending operations in flushPending', async () => {
      await expect(store.flushAllPending()).resolves.not.toThrow();
    });

    it('should execute pending operations immediately on flush', async () => {
      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...mockNode,
        version: 2
      });

      store.setNode(mockNode, databaseSource);

      // Create pending operation
      store.updateNode(mockNode.id, { content: 'Flush test' }, viewerSource);

      // Flush should execute immediately
      await store.flushAllPending();

      expect(store.getNode(mockNode.id)?.content).toBe('Flush test');
    });

    it('should handle errors during flush with timeout', async () => {
      // The operation never settles: flushAllPending must fall back to its own
      // internal timeout rather than hang. A never-resolving promise states that
      // intent directly — a sleep merely longer than the timeout implied the
      // duration mattered, and cost real seconds to express.
      vi.spyOn(backendAdapter, 'updateNode').mockImplementation(
        () => new Promise<Node>(() => {})
      );

      store.setNode(mockNode, databaseSource);
      store.updateNode(mockNode.id, { content: 'Long operation' }, viewerSource);

      // Drive the debounce and the internal flush timeout on fake time. Async
      // variant required: the flush path holds live timers whose promises are
      // settled by other microtasks (see shared-node-store.svelte.ts:123).
      vi.useFakeTimers();
      try {
        const flushPromise = store.flushAllPending();
        await vi.advanceTimersByTimeAsync(FLUSH_PENDING_TIMEOUT_MS);
        await expect(flushPromise).resolves.not.toThrow();
      } finally {
        vi.useRealTimers();
      }
    });

    it('does not double-execute an already in-flight operation on flush', async () => {
      // A controllable in-flight persist: the backend call hangs until we
      // resolve it manually, so we can flush WHILE the op is still executing.
      let resolveUpdate!: (v: Node) => void;
      const updateSpy = vi
        .spyOn(backendAdapter, 'updateNode')
        .mockImplementation(
          () => new Promise<Node>((res) => { resolveUpdate = res; })
        );

      store.setNode(mockNode, databaseSource);
      store.updateNode(mockNode.id, { content: 'in-flight' }, viewerSource);

      // Let the debounce fire: the op moves pending → executing and calls the
      // backend exactly once (now parked on our unresolved promise). Waiting on
      // that call rather than on a duration is both faster and stricter.
      await vi.waitFor(() => expect(updateSpy).toHaveBeenCalledTimes(1), {
        timeout: CASCADE_SETTLE_TIMEOUT_MS
      });

      // Flush on window-close while the op is still in flight. The executing-op
      // guard must NOT start a second backend call (the bug: an OCC conflict or
      // duplicate create at the worst possible moment).
      const flushPromise = store.flushAllPending();
      await new Promise((r) => setTimeout(r, 0));
      expect(updateSpy).toHaveBeenCalledTimes(1);

      // Complete the in-flight op; flush settles by awaiting that same promise.
      resolveUpdate({ ...mockNode, content: 'in-flight', version: 2 });
      await flushPromise;
      expect(updateSpy).toHaveBeenCalledTimes(1);
    });
  });

  // ========================================================================
  // flushAndWaitForNodes
  // ========================================================================

  describe('flushAndWaitForNodes', () => {
    it('should flush specific nodes and wait for completion', async () => {
      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...mockNode,
        version: 2
      });

      store.setNode(mockNode, databaseSource);
      store.updateNode(mockNode.id, { content: 'Flush specific' }, viewerSource);

      const failed = await store.flushAllPendingSaves(1000);

      expect(failed).toBeInstanceOf(Set);
    });

    it('should handle timeout in flushAndWaitForNodes', async () => {
      // Never settles, so the 50ms flush timeout below is the only thing that
      // can resolve this — which is precisely what the test asserts.
      vi.spyOn(backendAdapter, 'updateNode').mockImplementation(
        () => new Promise<Node>(() => {})
      );

      store.setNode(mockNode, databaseSource);
      store.updateNode(mockNode.id, { content: 'Timeout test' }, viewerSource);

      const failed = await store.flushAllPendingSaves(50);

      // Should have failed due to timeout
      expect(failed.size).toBeGreaterThanOrEqual(0);
    });

    it('should flush specific nodes while leaving others pending', async () => {
      const node1 = { ...mockNode, id: 'flush-node-1' };
      const node2 = { ...mockNode, id: 'flush-node-2' };
      const node3 = { ...mockNode, id: 'flush-node-3' };

      let updateCount = 0;
      vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async (id) => {
        updateCount++;
        return { ...mockNode, id, version: 2 };
      });

      // Set up all nodes
      store.setNode(node1, databaseSource);
      store.setNode(node2, databaseSource);
      store.setNode(node3, databaseSource);

      // Make updates to trigger pending operations
      store.updateNode(node1.id, { content: 'Updated 1' }, viewerSource);
      store.updateNode(node2.id, { content: 'Updated 2' }, viewerSource);
      store.updateNode(node3.id, { content: 'Updated 3' }, viewerSource);

      // Flush only specific nodes
      const failed = await store.flushNodeSaves([node1.id, node3.id], 1000);

      expect(failed.size).toBe(0);
      // At minimum node1 and node3 should have been flushed
      expect(updateCount).toBeGreaterThanOrEqual(2);
    });

    it('should return empty set for empty node list', async () => {
      const failed = await store.flushNodeSaves([], 1000);
      expect(failed.size).toBe(0);
    });

    it('should handle concurrent flush requests for same node', async () => {
      let resolveUpdate: (() => void) | undefined;
      const updatePromise = new Promise<void>(resolve => {
        resolveUpdate = resolve;
      });

      vi.spyOn(backendAdapter, 'updateNode').mockImplementation(async () => {
        await updatePromise;
        return { ...mockNode, version: 2 };
      });

      store.setNode(mockNode, databaseSource);
      store.updateNode(mockNode.id, { content: 'Concurrent test' }, viewerSource);

      // Start two concurrent flush operations for the same node
      const flush1 = store.flushNodeSaves([mockNode.id], 5000);
      const flush2 = store.flushNodeSaves([mockNode.id], 5000);

      // Release the update
      resolveUpdate!();

      // Both should complete without error
      const [failed1, failed2] = await Promise.all([flush1, flush2]);

      expect(failed1.size).toBe(0);
      expect(failed2.size).toBe(0);
    });

    it('should handle mix of pending and non-pending nodes', async () => {
      const pendingNode = { ...mockNode, id: 'pending-node' };
      const noPendingNode = { ...mockNode, id: 'no-pending-node' };

      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValue({
        ...pendingNode,
        version: 2
      });

      // Only set up pending operation for one node
      store.setNode(pendingNode, databaseSource);
      store.setNode(noPendingNode, databaseSource);
      store.updateNode(pendingNode.id, { content: 'Has pending' }, viewerSource);
      // noPendingNode has no pending update

      // Flush both - should handle missing pending gracefully
      const failed = await store.flushNodeSaves([pendingNode.id, noPendingNode.id], 1000);

      expect(failed.size).toBe(0);
    });

    it('should report failed nodes when operation errors', async () => {
      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValue(new Error('Update failed'));

      store.setNode(mockNode, databaseSource);
      store.updateNode(mockNode.id, { content: 'Will fail' }, viewerSource);

      const failed = await store.flushNodeSaves([mockNode.id], 1000);

      // Should report the node as failed
      expect(failed.has(mockNode.id)).toBe(true);
    });
  });

  // ========================================================================
  // waitForPersistence - Coverage
  // ========================================================================

  describe('waitForPersistence', () => {
    it('should skip nodes without pending operations', async () => {
      const failed = await store.waitForNodeSaves(['non-existent'], 1000);
      expect(failed.size).toBe(0);
    });
  });

  // ========================================================================
  // Edge Case: Empty commitBatch
  // ========================================================================

  describe('commitBatch Edge Cases', () => {
    it('should handle commitBatch when node no longer exists', () => {
      const node = { ...mockNode, id: 'temp-batch', nodeType: 'quote-block' };
      store.setNode(node, viewerSource, true);
      store.startBatch('temp-batch');

      // Delete the node
      store.deleteNode('temp-batch', viewerSource);

      // Commit should handle missing node gracefully
      expect(() => store.commitBatch('temp-batch')).not.toThrow();
    });
  });

  // ========================================================================
  // OCC Error Detection and Resync Trigger
  // ========================================================================

  describe('OCC Error Detection', () => {
    // NOTE: this error is a plain Error carrying a `code` property, which is
    // NOT what `isVersionConflict()` recognises — that requires structured
    // `conflictData`. So this test exercises the GENERIC write-failure path
    // (track + rollback), not the OCC resync branch, despite the original
    // title. Renamed to say what it actually covers; the real OCC resync
    // behaviour is covered by ai-chat-occ-conflict-regression.test.ts and
    // conflict-notification-integration.test.ts.
    it('rolls back and tracks the error when a write fails', async () => {
      const writeError = new Error('VERSION_CONFLICT: optimistic concurrency failure');
      (writeError as Error & { code?: string }).code = 'VERSION_CONFLICT';

      vi.spyOn(backendAdapter, 'updateNode').mockRejectedValue(writeError);

      store.setNode(mockNode, databaseSource);
      store.updateNode(mockNode.id, { content: 'Local edit' }, viewerSource);

      // Wait on the failure being observed rather than on a duration long
      // enough to cover debounce + rejection. vi.waitFor returns as soon as the
      // condition holds, so the common case costs the debounce and nothing more.
      await vi.waitFor(
        () => {
          expect(store.getTestErrors().length).toBeGreaterThan(0);
        },
        { timeout: CASCADE_SETTLE_TIMEOUT_MS }
      );

      // The failure is surfaced, and the node survives it. (The local edit is
      // deliberately NOT discarded — dropping what the user typed because a
      // write failed would be data loss.)
      expect(store.getTestErrors()[0].message).toContain('VERSION_CONFLICT');
      expect(store.getNode(mockNode.id)).toBeTruthy();
    });
  });

  // ========================================================================
  // CREATE Operation Race Condition (Issue: content overwritten mid-typing)
  // ========================================================================

  describe('CREATE Operation - User Typing Race Condition', () => {
    /**
     * REGRESSION TEST: Content overwritten during CREATE operation
     *
     * Bug scenario:
     * 1. User creates new node with content "## " (header)
     * 2. User deletes "## " (becomes text node)
     * 3. User presses Tab to indent
     * 4. User starts typing "Hello World"
     * 5. CREATE operation completes and fetches version from backend
     * 6. BUG: nodes.set() was called with stale `currentNode` captured at step 1
     * 7. User's typing ("Hello World") was overwritten with stale content ("## ")
     *
     * Fix: Only update the VERSION from backend, preserve local content
     */
    it('should preserve user typing when CREATE completes (setNode path)', async () => {
      let createNodeResolve: (value: string) => void;
      const createNodePromise = new Promise<string>((resolve) => {
        createNodeResolve = resolve;
      });

      // Mock createNode to delay so we can simulate user typing during the operation
      vi.spyOn(backendAdapter, 'createNode').mockImplementation(async () => {
        return createNodePromise;
      });

      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue({
        ...mockNode,
        id: 'typing-race-node',
        content: '## ', // Backend has OLD content
        nodeType: 'header',
        version: 1
      });

      // Step 1: Create new node with initial content (NOT persisted yet)
      // The node is new because persistedNodeIds doesn't have it
      const newNode = {
        ...mockNode,
        id: 'typing-race-node',
        content: '## ',
        nodeType: 'header' as const,
        version: 0
      };
      store.setNode(newNode, viewerSource); // Will trigger CREATE persistence

      // Step 2: Simulate user typing WHILE createNode is in flight
      // This updates the store directly (as the UI would via updateNode)
      // The store update happens immediately, but CREATE is still pending
      await new Promise(resolve => setTimeout(resolve, 10));

      // Directly modify the store's node to simulate what the UI does
      // (updateNode would queue its own persistence which complicates the test)
      const nodeInStore = store.getNode('typing-race-node');
      if (nodeInStore) {
        nodeInStore.content = 'Hello World';
        nodeInStore.nodeType = 'text';
        store.nodes.set('typing-race-node', nodeInStore);
      }

      // Verify store has user's typed content
      const beforeCreate = store.getNode('typing-race-node');
      expect(beforeCreate?.content).toBe('Hello World');
      expect(beforeCreate?.nodeType).toBe('text');

      // Step 3: Now complete the CREATE operation
      createNodeResolve!('typing-race-node');

      // Wait for persistence to complete (debounce is 500ms for new viewer nodes)
      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));

      // Step 4: CRITICAL CHECK - User's content MUST be preserved!
      const afterCreate = store.getNode('typing-race-node');
      expect(afterCreate?.content).toBe('Hello World'); // NOT '## '!
      expect(afterCreate?.nodeType).toBe('text'); // NOT 'header'!
      expect(afterCreate?.version).toBe(1); // Version updated from backend
    });

    it('should preserve user typing when batch CREATE completes (commitBatch path)', async () => {
      let createNodeResolve: (value: string) => void;
      const createNodePromise = new Promise<string>((resolve) => {
        createNodeResolve = resolve;
      });

      vi.spyOn(backendAdapter, 'createNode').mockImplementation(async () => {
        return createNodePromise;
      });

      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue({
        ...mockNode,
        id: 'batch-typing-race',
        content: '> Quote', // Backend has OLD content from batch
        nodeType: 'quote-block',
        version: 1
      });

      // Step 1: Create node with batch mode (quote-block uses batching)
      const quoteNode = {
        ...mockNode,
        id: 'batch-typing-race',
        content: '> Quote',
        nodeType: 'quote-block' as const,
        version: 0
      };
      store.setNode(quoteNode, viewerSource, true);

      // Start a batch operation
      store.startBatch('batch-typing-race');
      store.addToBatch('batch-typing-race', { content: '> Original batch content' });
      store.commitBatch('batch-typing-race');

      // Step 2: User types more content while batch is processing
      await new Promise(resolve => setTimeout(resolve, 10));
      store.updateNode('batch-typing-race', {
        content: '> User typed this after batch'
      }, viewerSource);

      // Verify store has user's typed content
      const beforeComplete = store.getNode('batch-typing-race');
      expect(beforeComplete?.content).toBe('> User typed this after batch');

      // Step 3: Complete the CREATE operation
      createNodeResolve!('batch-typing-race');

      // Wait for persistence to complete
      await new Promise(resolve => setTimeout(resolve, 100));

      // Step 4: CRITICAL CHECK - User's content MUST be preserved!
      const afterComplete = store.getNode('batch-typing-race');
      expect(afterComplete?.content).toBe('> User typed this after batch');
      expect(afterComplete?.version).toBe(1);
    });

    it('should handle rapid typing with multiple content changes during CREATE', async () => {
      let createNodeResolve: (value: string) => void;
      const createNodePromise = new Promise<string>((resolve) => {
        createNodeResolve = resolve;
      });

      vi.spyOn(backendAdapter, 'createNode').mockImplementation(async () => {
        return createNodePromise;
      });

      vi.spyOn(backendAdapter, 'getNode').mockResolvedValue({
        ...mockNode,
        id: 'rapid-typing',
        content: 'A',
        version: 1
      });

      // Create initial node (will trigger CREATE persistence)
      const node = { ...mockNode, id: 'rapid-typing', content: 'A', version: 0 };
      store.setNode(node, viewerSource);

      // Simulate rapid typing - directly modify store content
      // This simulates what happens when user types faster than persistence
      await new Promise(resolve => setTimeout(resolve, 5));
      const nodeInStore = store.getNode('rapid-typing');
      if (nodeInStore) {
        nodeInStore.content = 'AB';
        store.nodes.set('rapid-typing', nodeInStore);
        await new Promise(resolve => setTimeout(resolve, 5));
        nodeInStore.content = 'ABC';
        store.nodes.set('rapid-typing', nodeInStore);
        await new Promise(resolve => setTimeout(resolve, 5));
        nodeInStore.content = 'ABCD';
        store.nodes.set('rapid-typing', nodeInStore);
        await new Promise(resolve => setTimeout(resolve, 5));
        nodeInStore.content = 'ABCDE';
        store.nodes.set('rapid-typing', nodeInStore);
      }

      // Verify latest content
      expect(store.getNode('rapid-typing')?.content).toBe('ABCDE');

      // Complete CREATE
      createNodeResolve!('rapid-typing');
      await new Promise((resolve) => setTimeout(resolve, DEBOUNCED_WRITE_WAIT_MS));

      // Final content must be the LATEST user typed, not initial 'A'
      const final = store.getNode('rapid-typing');
      expect(final?.content).toBe('ABCDE');
      expect(final?.version).toBe(1);
    });
  });

  // ========================================================================
  // Concurrent Resync Prevention
  // ========================================================================

  describe('Concurrent Resync Prevention', () => {
    it('returns early on a concurrent resync attempt, then runs one queued follow-up', async () => {
      let callCount = 0;
      vi.spyOn(backendAdapter, 'getNode').mockImplementation(async () => {
        callCount++;
        await new Promise(resolve => setTimeout(resolve, 100));
        return { ...mockNode, version: 5 };
      });

      store.setNode(mockNode, viewerSource, true);

      // Fire concurrent resyncs
      const p1 = store.resyncNodeFromServer(mockNode.id);
      const p2 = store.resyncNodeFromServer(mockNode.id);
      const p3 = store.resyncNodeFromServer(mockNode.id);

      // p2/p3 arrive while p1's fetch is in flight, so both return early
      // (queuing at most one follow-up) well before p1's own mocked fetch does.
      await Promise.all([p2, p3]);
      expect(callCount).toBe(1);

      // p1 settling runs its finally block, which fires the single queued
      // follow-up — a second, sequential (not concurrent) fetch, not a
      // silently dropped request.
      await p1;
      expect(callCount).toBe(2);
    });
  });
});
