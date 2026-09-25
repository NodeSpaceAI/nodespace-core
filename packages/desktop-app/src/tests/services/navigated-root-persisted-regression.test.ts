/**
 * Regression coverage for a root node opened by navigation being inserted
 * into `SharedNodeStore` without persisted tracking.
 *
 * `NavigationService.resolveNodeTarget()` fetches a node that is not yet in
 * the store and inserts it with a `database` source plus `skipPersistence`.
 * `skipPersistence` used to outrank the database source in
 * `determinePersistenceBehavior`, so the node was never added to
 * `persistedNodeIds`. The viewer's `loadChildrenTree()` then skips the
 * already-cached root, so nothing marked it later either — and the first edit
 * took the create path, which the backend rejected as a duplicate insert.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { getNavigationService } from '$lib/services/navigation-service';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import type { Node } from '$lib/types';

const ROOT_ID = 'navigated-root-person';

function personNode(overrides: Partial<Node> = {}): Node {
  return {
    id: ROOT_ID,
    nodeType: 'person',
    content: '',
    createdAt: '2026-01-01T00:00:00.000Z',
    modifiedAt: '2026-01-01T00:00:00.000Z',
    version: 1,
    properties: { first_name: 'Ada', last_name: 'Lovelace', email: 'ada@example.com' },
    ...overrides
  } as Node;
}

describe('navigated-to root node — persisted tracking', () => {
  beforeEach(() => {
    sharedNodeStore.clearAll();
  });

  afterEach(() => {
    sharedNodeStore.clearAll();
    vi.restoreAllMocks();
  });

  it('marks the root persisted, so an edit takes the update path', async () => {
    vi.spyOn(backendAdapter, 'getNode').mockResolvedValue(personNode());
    const { id, nodeType, content, version, createdAt, modifiedAt, properties } = personNode();
    vi.spyOn(backendAdapter, 'getChildrenTree').mockResolvedValue({
      id,
      nodeType,
      content,
      version,
      createdAt,
      modifiedAt,
      properties,
      children: []
    });
    vi.spyOn(backendAdapter, 'getMentioningContainers').mockResolvedValue([]);
    const createNodeSpy = vi.spyOn(backendAdapter, 'createNode').mockResolvedValue(ROOT_ID);
    const updateNodeSpy = vi
      .spyOn(backendAdapter, 'updateNode')
      .mockImplementation(async (_id, version, update) =>
        personNode({ ...(update as Partial<Node>), version: version + 1 })
      );

    // Navigation inserts the root before the viewer loads its tree.
    const target = await getNavigationService().resolveNodeTarget(ROOT_ID);
    expect(target?.nodeId).toBe(ROOT_ID);
    await sharedNodeStore.loadChildrenTree(ROOT_ID);

    expect(sharedNodeStore.isNodePersisted(ROOT_ID)).toBe(true);

    sharedNodeStore.updateNode(
      ROOT_ID,
      { properties: { first_name: 'Ada', last_name: 'Byron', email: 'ada@example.com' } },
      { type: 'viewer', viewerId: 'person-viewer' }
    );

    await vi.waitFor(() => {
      expect(updateNodeSpy).toHaveBeenCalledWith(ROOT_ID, 1, expect.anything());
    });
    expect(createNodeSpy).not.toHaveBeenCalled();
  });
});
