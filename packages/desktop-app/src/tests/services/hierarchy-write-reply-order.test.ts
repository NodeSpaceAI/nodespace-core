/**
 * The client that makes a hierarchy write never receives that write's own
 * relationship events (same-origin echo suppression), so the store's order keys
 * reach it only in the write's reply. After its own move or create resolves,
 * the client's structureTree must hold the store's keys — for the written edge
 * and for every sibling a re-spread rewrote — not the ones it computed locally.
 *
 * Uses the REAL structureTree and SharedNodeStore, and spies on the shared
 * backendAdapter to play the daemon's replies.
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
import { backendAdapter, type ChildPlacement } from '$lib/services/backend-adapter';
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

describe("a hierarchy write's reply carries the store's order keys to its caller", () => {
  let service: ReactiveNodeService;
  let sharedNodeStore: SharedNodeStore;

  beforeEach(() => {
    SharedNodeStore.resetInstance();
    SimplePersistenceCoordinator.resetInstance();
    sharedNodeStore = SharedNodeStore.getInstance();
    structureTree.clear();
    focusManager.clearEditing();
    vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(null);

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
    vi.restoreAllMocks();
  });

  function addPersistedNode(id: string, parentId: string, order: number) {
    sharedNodeStore.setNode(makeNode(id), { type: 'database', reason: 'test' });
    structureTree.addInMemoryRelationship(parentId, id, order);
  }

  const withOrders = (parentId: string) =>
    structureTree.getChildrenWithOrder(parentId).map((c) => [c.nodeId, c.order]);

  it('an indent applies the moved edge and re-spread sibling keys from the reply', async () => {
    // root: a (1), b (2); a has children a1 (1), a2 (1.0000001) — a nearly
    // collapsed gap, so the daemon re-spreads a's children when b lands there.
    addPersistedNode('a', 'root', 1);
    addPersistedNode('b', 'root', 2);
    addPersistedNode('a1', 'a', 1);
    addPersistedNode('a2', 'a', 1.0000001);

    const placement: ChildPlacement = {
      parentId: 'a',
      order: 3,
      respread: [
        { nodeId: 'a1', order: 1 },
        { nodeId: 'a2', order: 2 }
      ]
    };
    const moveNodeSpy = vi
      .spyOn(backendAdapter, 'moveNode')
      .mockImplementation(async (id) => ({ node: makeNode(id, 2), placement }));

    expect(await service.indentNode('b')).toBe(true);
    await waitForPendingMoveOperations();

    expect(moveNodeSpy).toHaveBeenCalledWith('b', 1, 'a', null);
    expect(withOrders('a')).toEqual([
      ['a1', 1],
      ['a2', 2],
      ['b', 3]
    ]);
    expect(sharedNodeStore.getNode('b')?.version).toBe(2);
  });

  it("an outdent applies the store's keys to the node and to the trailing siblings it adopts", async () => {
    // root: a (1); a has children b (1), c (2). Outdenting b moves it under root
    // after a, and c — the sibling below it — becomes b's child in one transfer.
    addPersistedNode('a', 'root', 1);
    addPersistedNode('b', 'a', 1);
    addPersistedNode('c', 'a', 2);

    vi.spyOn(backendAdapter, 'moveNode').mockImplementation(async (id) => ({
      node: makeNode(id, 2),
      placement: { parentId: 'root', order: 1.5, respread: [] }
    }));
    const moveChildrenSpy = vi
      .spyOn(backendAdapter, 'moveChildrenToParent')
      .mockImplementation(async (_parentId, children) => ({
        nodes: children.map((c) => makeNode(c.id, 2)),
        orders: children.map((c) => ({ nodeId: c.id, order: 42 }))
      }));

    expect(await service.outdentNode('b')).toBe(true);
    await waitForPendingMoveOperations();

    expect(moveChildrenSpy).toHaveBeenCalledWith('b', [{ id: 'c', version: 1 }]);
    expect(withOrders('root')).toEqual([
      ['a', 1],
      ['b', 1.5]
    ]);
    expect(withOrders('b')).toEqual([['c', 42]]);
  });

  it("a create applies the new edge's key and the re-spread sibling keys from the reply", async () => {
    addPersistedNode('a', 'root', 1);
    addPersistedNode('b', 'root', 1.0000001);

    // Enter after `a`: the local tree places `x` at a midpoint of its own choosing.
    structureTree.addInMemoryRelationship('root', 'x', 1.00000005);
    sharedNodeStore.setNode(
      { ...makeNode('x'), insertPosition: { type: 'after', siblingId: 'a' } } as Node,
      { type: 'viewer', viewerId: 'test-viewer' }
    );

    const createNodeSpy = vi.spyOn(backendAdapter, 'createNode').mockResolvedValue({
      id: 'x',
      placement: {
        parentId: 'root',
        order: 1.5,
        respread: [
          { nodeId: 'a', order: 1 },
          { nodeId: 'b', order: 2 }
        ]
      }
    });

    await sharedNodeStore.flushAllPendingSaves();

    expect(createNodeSpy).toHaveBeenCalledTimes(1);
    expect(withOrders('root')).toEqual([
      ['a', 1],
      ['x', 1.5],
      ['b', 2]
    ]);
  });
});
