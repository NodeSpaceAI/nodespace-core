/**
 * Per-node write sequencing in SharedNodeStore:
 *
 * 1. A write never waits on a move that waits on it. A move (indent/outdent)
 *    flushes the node's pending writes before sending, and writes wait for a
 *    pending move before sending — so a write inside that flush must not wait
 *    on the move, or both stall until the flush's 5 s timeout.
 * 2. A generic write's non-content change survives a later write for the same
 *    node queued behind the same in-flight RPC.
 * 3. A version conflict on staged typed fields flushed inside another write
 *    raises one notification: the calling write does not send, and callbacks
 *    of staged fields the conflict drops are settled.
 *
 * Uses the REAL structureTree and ReactiveNodeService so moves run the same
 * flush they run in the app, and spies on the shared backendAdapter.
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  createReactiveNodeService,
  type ReactiveNodeService
} from '$lib/services/reactive-node-service.svelte';
import {
  SharedNodeStore,
  SimplePersistenceCoordinator
} from '$lib/services/shared-node-store.svelte';
import { structureTree } from '$lib/stores/reactive-structure-tree.svelte';
import { focusManager } from '$lib/services/focus-manager.svelte';
import { waitForPendingMoveOperations } from '$lib/services/pending-operations';
import { backendAdapter } from '$lib/services/backend-adapter';
import { conflictNotifications } from '$lib/stores/conflict-notifications.svelte';
import type { Node, TaskNode } from '$lib/types';

/** Well under the 5 s flush timeout a wait cycle stalls for. */
const NO_STALL_MS = 1500;

const dbSource = { type: 'database' as const, reason: 'test' };
const viewerSource = { type: 'viewer' as const, viewerId: 'test-viewer' };

function makeNode(id: string, nodeType = 'text', version = 1): Node {
  return {
    id,
    nodeType,
    content: `Content ${id}`,
    version,
    properties: {},
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    ...(nodeType === 'task' ? { status: 'open' } : {})
  } as Node;
}

function typedResponse(id: string, version: number, payload: object): TaskNode {
  return { ...makeNode(id, 'task', version), ...payload } as unknown as TaskNode;
}

function versionConflict(node: Node) {
  const error = new Error('VERSION_CONFLICT: optimistic concurrency failure') as Error & {
    code: string;
    conflictData: { node_id: string; expected: number; actual: number; current_node: Node };
  };
  error.code = 'VERSION_CONFLICT';
  error.conflictData = {
    node_id: node.id,
    expected: node.version,
    actual: node.version + 1,
    current_node: { ...node, version: node.version + 1 }
  };
  return error;
}

/** A promise plus the function that resolves it. */
function deferred<T = void>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

describe('per-node write sequencing', () => {
  let service: ReactiveNodeService;
  let store: SharedNodeStore;

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    SimplePersistenceCoordinator.resetInstance();
    store = SharedNodeStore.getInstance();
    structureTree.clear();
    focusManager.clearEditing();
    conflictNotifications.dismissAll();
    vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(null);

    service = createReactiveNodeService({
      focusRequested: vi.fn(),
      hierarchyChanged: vi.fn(),
      nodeCreated: vi.fn(),
      nodeDeleted: vi.fn()
    });
  });

  afterEach(async () => {
    await waitForPendingMoveOperations();
    service.destroy();
    structureTree.clear();
    focusManager.clearEditing();
    conflictNotifications.dismissAll();
    vi.restoreAllMocks();
  });

  function addPersistedNode(id: string, parentId: string | null, order: number, type = 'text') {
    store.setNode(makeNode(id, type), dbSource);
    if (parentId) structureTree.addInMemoryRelationship(parentId, id, order);
  }

  describe('a write never waits on a move that waits on it', () => {
    it('sends typed fields staged during a pending create before the indent that flushes the create', async () => {
      addPersistedNode('parent', null, 1);
      addPersistedNode('sibling', 'parent', 1);

      // Enter: a viewer-sourced task node whose CREATE is pending.
      structureTree.addInMemoryRelationship('parent', 'task-1', 2);
      store.setNode(makeNode('task-1', 'task'), viewerSource);
      expect(store.isNodePersisted('task-1')).toBe(false);

      const createGate = deferred();
      const createSpy = vi.spyOn(backendAdapter, 'createNode').mockImplementation(async (input) => {
        await createGate.promise;
        return input.id ?? '';
      });
      const typedSpy = vi
        .spyOn(backendAdapter, 'updateTaskNode')
        .mockImplementation(async (id, version, payload) => typedResponse(id, version + 1, payload));
      const moveSpy = vi
        .spyOn(backendAdapter, 'moveNode')
        .mockImplementation(async (id, version) => makeNode(id, 'task', version + 1));

      // The CREATE is now in flight.
      void store.flushNodeSaves(['task-1']);
      await vi.waitFor(() => expect(createSpy).toHaveBeenCalled());
      expect(store.isNodePersistenceExecuting('task-1')).toBe(true);

      // A checkbox click stages a typed field; Tab falls through to a MOVE
      // that flushes the in-flight CREATE.
      store.updateTaskNode('task-1', { status: 'done' }, viewerSource);
      expect(await service.indentNode('task-1')).toBe(true);

      createGate.resolve();

      await vi.waitFor(() => expect(moveSpy).toHaveBeenCalledTimes(1), { timeout: NO_STALL_MS });
      expect(typedSpy).toHaveBeenCalledTimes(1);
      expect(typedSpy.mock.calls[0][2]).toEqual({ status: 'done' });
      expect(typedSpy.mock.invocationCallOrder[0]).toBeLessThan(moveSpy.mock.invocationCallOrder[0]);
    });

    it('sends a typed write made between two quick indents after the first move and before the second', async () => {
      addPersistedNode('parent', null, 1);
      addPersistedNode('x', 'parent', 1);
      addPersistedNode('y', 'x', 1);
      addPersistedNode('task-2', 'parent', 2, 'task');

      const firstMoveGate = deferred();
      const moveSpy = vi
        .spyOn(backendAdapter, 'moveNode')
        .mockImplementation(async (id, version) => {
          if (moveSpy.mock.calls.length === 1) await firstMoveGate.promise;
          return makeNode(id, 'task', version + 1);
        });
      const typedSpy = vi
        .spyOn(backendAdapter, 'updateTaskNode')
        .mockImplementation(async (id, version, payload) => typedResponse(id, version + 1, payload));

      // Tab, Tab: under x, then under y.
      expect(await service.indentNode('task-2')).toBe(true);
      await vi.waitFor(() => expect(moveSpy).toHaveBeenCalledTimes(1));
      expect(await service.indentNode('task-2')).toBe(true);

      // Checkbox click while the first move is still in flight.
      store.updateTaskNode('task-2', { status: 'done' }, viewerSource);

      firstMoveGate.resolve();

      await vi.waitFor(() => expect(moveSpy).toHaveBeenCalledTimes(2), { timeout: NO_STALL_MS });
      expect(typedSpy).toHaveBeenCalledTimes(1);
      const [firstMove, secondMove] = moveSpy.mock.invocationCallOrder;
      const typed = typedSpy.mock.invocationCallOrder[0];
      expect(firstMove).toBeLessThan(typed);
      expect(typed).toBeLessThan(secondMove);
      // Sent at the version the first move returned, so it doesn't conflict.
      expect(typedSpy.mock.calls[0][1]).toBe(2);
    });

    it('sends a pending content edit before the indent that flushes it', async () => {
      addPersistedNode('parent', null, 1);
      addPersistedNode('sibling', 'parent', 1);
      addPersistedNode('text-1', 'parent', 2);

      const updateSpy = vi
        .spyOn(backendAdapter, 'updateNode')
        .mockImplementation(async (id, version) => makeNode(id, 'text', version + 1));
      const moveSpy = vi
        .spyOn(backendAdapter, 'moveNode')
        .mockImplementation(async (id, version) => makeNode(id, 'text', version + 1));

      // Typing leaves a debounced write pending; Tab flushes it.
      store.updateNode('text-1', { content: 'typed' }, viewerSource);
      expect(await service.indentNode('text-1')).toBe(true);

      await vi.waitFor(() => expect(moveSpy).toHaveBeenCalledTimes(1), { timeout: NO_STALL_MS });
      expect(updateSpy).toHaveBeenCalledTimes(1);
      expect(updateSpy.mock.invocationCallOrder[0]).toBeLessThan(
        moveSpy.mock.invocationCallOrder[0]
      );
      expect(moveSpy.mock.calls[0][1]).toBe(2);
    });
  });

  it('keeps a queued non-content change when a later write for the same node queues behind it', async () => {
    addPersistedNode('text-2', null, 1);

    const firstWriteGate = deferred();
    const updateSpy = vi
      .spyOn(backendAdapter, 'updateNode')
      .mockImplementation(async (id, version) => {
        if (updateSpy.mock.calls.length === 1) await firstWriteGate.promise;
        return makeNode(id, 'text', version + 1);
      });

    // Typing, flushed into an in-flight RPC.
    store.updateNode('text-2', { content: 'a' }, viewerSource);
    void store.flushNodeSaves(['text-2']);
    await vi.waitFor(() => expect(updateSpy).toHaveBeenCalledTimes(1));

    // A property change via a shortcut, then more typing — both queue behind it.
    store.updateNode('text-2', { properties: { 'custom:flag': true } }, viewerSource);
    store.updateNode('text-2', { content: 'ab' }, viewerSource);

    firstWriteGate.resolve();
    await store.flushAllPendingSaves();

    const payloads = updateSpy.mock.calls.map(([, , payload]) => payload);
    expect(payloads).toEqual([
      { content: 'a' },
      { properties: { 'custom:flag': true } },
      { content: 'ab' }
    ]);
  });

  it('raises one notification for a conflict on typed fields flushed inside another write, and settles dropped callbacks', async () => {
    addPersistedNode('task-3', null, 1, 'task');
    focusManager.focusNode('task-3', 'default');
    expect(focusManager.isNodeEditing('task-3')).toBe(true);

    const firstWriteGate = deferred();
    const updateSpy = vi
      .spyOn(backendAdapter, 'updateNode')
      .mockImplementation(async (id, version) => {
        if (updateSpy.mock.calls.length === 1) await firstWriteGate.promise;
        return makeNode(id, 'task', version + 1);
      });

    const sentFieldError = vi.fn();
    const droppedFieldError = vi.fn();
    const typedSpy = vi.spyOn(backendAdapter, 'updateTaskNode').mockImplementation(async () => {
      // A field staged while the typed RPC is in flight.
      store.updateTypedNode('task-3', 'task', { priority: 'high' }, viewerSource, {
        onPersistError: droppedFieldError
      });
      throw versionConflict(store.getNode('task-3')!);
    });

    // An in-flight content write; behind it a typed write, which a generic
    // property write then replaces — the generic write flushes the typed fields.
    store.updateNode('task-3', { content: '- [ ] edited' }, viewerSource);
    void store.flushNodeSaves(['task-3']);
    await vi.waitFor(() => expect(updateSpy).toHaveBeenCalledTimes(1));
    store.updateTypedNode('task-3', 'task', { status: 'done' }, viewerSource, {
      onPersistError: sentFieldError
    });
    store.updateNode('task-3', { properties: { 'custom:flag': true } }, viewerSource);

    firstWriteGate.resolve();
    await store.flushAllPendingSaves();

    expect(typedSpy).toHaveBeenCalledTimes(1);
    // The generic write that ran the flush did not send on the stale version.
    expect(updateSpy).toHaveBeenCalledTimes(1);
    const mismatches = conflictNotifications.notifications.filter(
      (n) => n.conflictType === 'version-mismatch'
    );
    expect(mismatches).toHaveLength(1);
    expect(droppedFieldError).toHaveBeenCalledTimes(1);
    // The conflicting send itself resolves through the conflict, not onPersistError.
    expect(sentFieldError).not.toHaveBeenCalled();
  });
});
