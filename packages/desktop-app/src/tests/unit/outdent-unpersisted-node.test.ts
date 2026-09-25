/**
 * outdentNode on a not-yet-persisted node (Shift+Tab inside the persistence
 * debounce window) must carry the same hierarchy changes to the backend as the
 * already-persisted MOVE path:
 *
 * 1. Outdenting to root: the deferred CREATE derives its parentId from
 *    structureTree at execution time, so the tree must report the node as a
 *    root — otherwise the CREATE lands under the OLD parent.
 * 2. Trailing siblings under the old parent become children of the outdented
 *    node, both in structureTree and in the backend — in one atomic RPC.
 * 3. When that sibling transfer fails after the node's own CREATE/MOVE has
 *    committed, only the siblings are rolled back, so the local tree keeps
 *    matching the backend, and the failure is surfaced.
 *
 * Uses the REAL structureTree (not a mock) so the CREATE's parentId is read
 * from the same tree the service mutates, and spies on the shared
 * backendAdapter object to observe what actually reaches the backend.
 */

import { describe, it, expect, beforeEach, afterEach, vi, type MockInstance } from 'vitest';
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
import type { Node } from '$lib/types';

function makeNode(id: string, version = 1): Node {
  return {
    id,
    nodeType: 'text',
    content: `Content ${id}`,
    version,
    properties: {},
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString()
  };
}

describe('outdentNode propagates root reparenting and sibling transfer', () => {
  let service: ReactiveNodeService;
  let sharedNodeStore: SharedNodeStore;
  let createNodeSpy: MockInstance<typeof backendAdapter.createNode>;
  let moveNodeSpy: MockInstance<typeof backendAdapter.moveNode>;
  let moveChildrenSpy: MockInstance<typeof backendAdapter.moveChildrenToParent>;

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    SimplePersistenceCoordinator.resetInstance();
    sharedNodeStore = SharedNodeStore.getInstance();
    structureTree.clear();
    focusManager.clearEditing();

    createNodeSpy = vi
      .spyOn(backendAdapter, 'createNode')
      .mockImplementation(async (input) => input.id ?? '');
    vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(null);
    moveNodeSpy = vi
      .spyOn(backendAdapter, 'moveNode')
      .mockImplementation(async (id) => makeNode(id, 2));
    moveChildrenSpy = vi
      .spyOn(backendAdapter, 'moveChildrenToParent')
      .mockImplementation(async (_parentId, children) => children.map((c) => makeNode(c.id, 2)));
    conflictNotifications.dismissAll();

    service = createReactiveNodeService({
      focusRequested: vi.fn(),
      hierarchyChanged: vi.fn(),
      nodeCreated: vi.fn(),
      nodeDeleted: vi.fn()
    });
  });

  afterEach(() => {
    service.destroy();
    structureTree.clear();
    focusManager.clearEditing();
    conflictNotifications.dismissAll();
    vi.restoreAllMocks();
  });

  function addPersistedNode(id: string, parentId: string | null, order: number) {
    sharedNodeStore.setNode(makeNode(id), { type: 'database', reason: 'test' });
    if (parentId) structureTree.addInMemoryRelationship(parentId, id, order);
  }

  /** Create a node the way Enter does: viewer-sourced, debounced CREATE pending, focused. */
  function addUnpersistedFocusedNode(id: string, parentId: string, order: number) {
    structureTree.addInMemoryRelationship(parentId, id, order);
    sharedNodeStore.setNode(makeNode(id), { type: 'viewer', viewerId: 'test-viewer' });
    focusManager.focusNode(id, 'default');
    expect(sharedNodeStore.isNodePersisted(id)).toBe(false);
    expect(sharedNodeStore.hasPendingSave(id)).toBe(true);
  }

  /** Fire the debounced CREATE and let any tracked move operation finish. */
  async function settle() {
    await sharedNodeStore.flushAllPendingSaves();
    await waitForPendingMoveOperations();
  }

  /** [newParentId, childIds] of each atomic child transfer sent to the backend */
  function childTransfers() {
    return moveChildrenSpy.mock.calls.map(([parentId, children]) => [
      parentId,
      children.map((c) => c.id)
    ]);
  }

  function childTransferFailures() {
    return conflictNotifications.notifications.filter(
      (n) => n.conflictType === 'child-transfer-failure'
    );
  }

  /** parentId of each CREATE sent to the backend for `id` */
  function createParentsFor(id: string) {
    return createNodeSpy.mock.calls
      .map(([input]) => input)
      .filter((input) => input.id === id)
      .map((input) => ('parentId' in input ? input.parentId : undefined));
  }

  it('outdenting to root makes the CREATE land with a null (root) parent, not the old parent', async () => {
    addPersistedNode('parent', null, 1);
    addUnpersistedFocusedNode('child', 'parent', 1);

    expect(await service.outdentNode('child')).toBe(true);

    expect(structureTree.getParent('child')).toBeNull();
    expect(structureTree.getChildren('parent')).not.toContain('child');

    await settle();

    expect(createParentsFor('child')).toEqual([null]);
    expect(sharedNodeStore.isNodePersisted('child')).toBe(true);
    expect(moveNodeSpy).not.toHaveBeenCalled();
  });

  it('transfers trailing siblings to the outdented node in structureTree and the backend', async () => {
    addPersistedNode('grandparent', null, 1);
    addPersistedNode('parent', 'grandparent', 1);
    addPersistedNode('before', 'parent', 1);
    // Enter in the middle of the list: the new node lands between 'before' and the trailing siblings
    addUnpersistedFocusedNode('child', 'parent', 2);
    addPersistedNode('after1', 'parent', 3);
    addPersistedNode('after2', 'parent', 4);

    expect(await service.outdentNode('child')).toBe(true);

    expect(structureTree.getParent('child')).toBe('grandparent');
    expect(structureTree.getChildren('parent')).toEqual(['before']);
    expect(structureTree.getChildren('child')).toEqual(['after1', 'after2']);

    await settle();

    expect(createParentsFor('child')).toEqual(['grandparent']);

    // The node itself is CREATEd under the right parent — no MOVE for it. Its
    // trailing siblings are moved under it in ONE atomic call, in their original
    // order, only after the CREATE has landed.
    expect(moveNodeSpy).not.toHaveBeenCalled();
    expect(childTransfers()).toEqual([['child', ['after1', 'after2']]]);
    expect(createNodeSpy.mock.invocationCallOrder[0]).toBeLessThan(
      moveChildrenSpy.mock.invocationCallOrder[0]
    );
  });

  it('outdenting an already-persisted node to root also reparents it in structureTree', async () => {
    // Same root guard, persisted MOVE path: a stale tree made the NEXT outdent resolve the
    // stale grandparent and nest the node instead of promoting it.
    addPersistedNode('parent', null, 1);
    addPersistedNode('child', 'parent', 1);

    expect(await service.outdentNode('child')).toBe(true);

    expect(structureTree.getParent('child')).toBeNull();
    expect(structureTree.getChildren('parent')).not.toContain('child');

    await settle();

    expect(moveNodeSpy.mock.calls.map(([id, , parentId]) => [id, parentId])).toEqual([
      ['child', null]
    ]);
  });

  it('an in-flight CREATE is MOVEd to the new parent once it lands, before its siblings are moved under it', async () => {
    addPersistedNode('grandparent', null, 1);
    addPersistedNode('parent', 'grandparent', 1);
    addUnpersistedFocusedNode('child', 'parent', 1);
    addPersistedNode('after1', 'parent', 2);

    // Start the CREATE and hold it mid-RPC: it has already read the OLD parent.
    let releaseCreate!: () => void;
    createNodeSpy.mockImplementation(
      (input) => new Promise((resolve) => (releaseCreate = () => resolve(input.id ?? '')))
    );
    const firstFlush = sharedNodeStore.flushAllPendingSaves();
    await vi.waitFor(() => expect(sharedNodeStore.isNodePersistenceExecuting('child')).toBe(true));
    expect(createParentsFor('child')).toEqual(['parent']);

    expect(await service.outdentNode('child')).toBe(true);
    expect(structureTree.getChildren('child')).toEqual(['after1']);

    // Nothing may be moved under 'child' before it exists in the backend — drain a full
    // macrotask so a MOVE scheduled a few ticks later would still be caught
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(moveNodeSpy).not.toHaveBeenCalled();
    expect(moveChildrenSpy).not.toHaveBeenCalled();

    releaseCreate();
    await firstFlush;
    await settle();

    // The node converges on the new parent even though its CREATE used the old one
    expect(moveNodeSpy.mock.calls.map(([id, , parentId]) => [id, parentId])).toEqual([
      ['child', 'grandparent']
    ]);
    expect(childTransfers()).toEqual([['child', ['after1']]]);
    expect(moveNodeSpy.mock.invocationCallOrder[0]).toBeLessThan(
      moveChildrenSpy.mock.invocationCallOrder[0]
    );
  });

  it('rolls back an outdent to root when the CREATE re-trigger is declined', async () => {
    addPersistedNode('parent', null, 1);
    addUnpersistedFocusedNode('child', 'parent', 1);
    vi.spyOn(sharedNodeStore, 'setNode').mockReturnValue(false);

    expect(await service.outdentNode('child')).toBe(false);

    expect(structureTree.getParent('child')).toBe('parent');
    expect(service.rootNodeIds).not.toContain('child');
    expect(moveNodeSpy).not.toHaveBeenCalled();
  });

  it('a failed MOVE restores the node and its transferred siblings under the old parent, in order', async () => {
    addPersistedNode('grandparent', null, 1);
    addPersistedNode('parent', 'grandparent', 1);
    addPersistedNode('child', 'parent', 1);
    addPersistedNode('after1', 'parent', 2);
    addPersistedNode('after2', 'parent', 3);
    moveNodeSpy.mockRejectedValue(new Error('move rejected'));

    expect(await service.outdentNode('child')).toBe(true);
    expect(structureTree.getChildren('child')).toEqual(['after1', 'after2']);

    await settle();

    expect(structureTree.getParent('child')).toBe('parent');
    expect(structureTree.getChildren('parent')).toEqual(['child', 'after1', 'after2']);
    expect(structureTree.getChildren('child')).toEqual([]);
    // The node's own MOVE failed first, so the sibling transfer was never attempted
    expect(moveChildrenSpy).not.toHaveBeenCalled();
  });

  it('a new node whose sibling transfer fails stays under its new parent; only the siblings return', async () => {
    addPersistedNode('grandparent', null, 1);
    addPersistedNode('parent', 'grandparent', 1);
    addPersistedNode('before', 'parent', 1);
    addUnpersistedFocusedNode('child', 'parent', 2);
    addPersistedNode('after1', 'parent', 3);
    addPersistedNode('after2', 'parent', 4);
    moveChildrenSpy.mockRejectedValue(new Error('version conflict'));

    expect(await service.outdentNode('child')).toBe(true);
    await settle();

    // The re-triggered CREATE committed under 'grandparent' — the local tree must agree
    expect(createParentsFor('child')).toEqual(['grandparent']);
    expect(structureTree.getParent('child')).toBe('grandparent');
    expect(structureTree.getChildren('parent')).toEqual(['before', 'after1', 'after2']);
    expect(structureTree.getChildren('child')).toEqual([]);
    expect(childTransferFailures().map((n) => n.nodeId)).toEqual(['child']);
  });

  it('a saved node whose own MOVE committed but sibling transfer fails stays moved; only the siblings return', async () => {
    addPersistedNode('grandparent', null, 1);
    addPersistedNode('parent', 'grandparent', 1);
    addPersistedNode('child', 'parent', 1);
    addPersistedNode('after1', 'parent', 2);
    addPersistedNode('after2', 'parent', 3);
    moveChildrenSpy.mockRejectedValue(new Error('version conflict'));

    expect(await service.outdentNode('child')).toBe(true);
    await settle();

    expect(moveNodeSpy.mock.calls.map(([id, , parentId]) => [id, parentId])).toEqual([
      ['child', 'grandparent']
    ]);
    expect(structureTree.getParent('child')).toBe('grandparent');
    expect(structureTree.getChildren('parent')).toEqual(['after1', 'after2']);
    expect(structureTree.getChildren('child')).toEqual([]);
    expect(childTransferFailures().map((n) => n.nodeId)).toEqual(['child']);
  });

  it('a failed MOVE of the node itself is not reported as a child-transfer failure', async () => {
    addPersistedNode('grandparent', null, 1);
    addPersistedNode('parent', 'grandparent', 1);
    addPersistedNode('child', 'parent', 1);
    addPersistedNode('after1', 'parent', 2);
    moveNodeSpy.mockRejectedValue(new Error('move rejected'));

    await service.outdentNode('child');
    await settle();

    expect(childTransferFailures()).toEqual([]);
  });

  it('rolls back an indent of a root node when the CREATE re-trigger is declined', async () => {
    // Empty text nodes initialize as unpersisted placeholders with a pending CREATE
    service.initializeNodes([makeNode('first'), { ...makeNode('second'), content: '' }]);
    expect(sharedNodeStore.isNodePersisted('second')).toBe(false);
    vi.spyOn(sharedNodeStore, 'setNode').mockReturnValue(false);

    expect(await service.indentNode('second')).toBe(false);

    expect(structureTree.getParent('second')).toBeNull();
    expect(structureTree.getChildren('first')).not.toContain('second');
    expect(service.rootNodeIds).toContain('second');
  });

  it('with no trailing siblings, issues no MOVE — the re-triggered CREATE carries the new parent', async () => {
    addPersistedNode('grandparent', null, 1);
    addPersistedNode('parent', 'grandparent', 1);
    addUnpersistedFocusedNode('child', 'parent', 1);

    expect(await service.outdentNode('child')).toBe(true);
    await settle();

    expect(moveNodeSpy).not.toHaveBeenCalled();
    expect(createParentsFor('child')).toEqual(['grandparent']);
  });
});
