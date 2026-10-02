/**
 * Optimistic collection creation.
 *
 * A newly created collection has zero members, and the collections tree hides
 * empty collections — so a create that waited on a backend round-trip and then
 * reloaded made the new collection invisible on the very refresh meant to
 * reveal it. `createCollection` therefore inserts the entry into local state
 * immediately (exempt from the hide-empty filter), reconciles its temporary id
 * with the backend's real one on success, and rolls it back out on failure.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() })
}));

import type { CollectionInfo } from '$lib/services/collection-service';

const mockCreateCollection = vi.fn<(name: string, description?: string) => Promise<string>>();
const mockGetAllCollections = vi.fn<() => Promise<CollectionInfo[]>>();
vi.mock('$lib/services/collection-service', () => ({
  collectionService: {
    createCollection: (name: string, description?: string) =>
      mockCreateCollection(name, description),
    getAllCollections: () => mockGetAllCollections()
  }
}));

import { collectionsData } from '$lib/stores/collections.svelte';

function makeCollection(id: string, name: string, memberCount: number): CollectionInfo {
  return {
    lifecycleStatus: 'active',
    id,
    content: name,
    nodeType: 'collection',
    createdAt: new Date().toISOString(),
    modifiedAt: new Date().toISOString(),
    version: 1,
    properties: {},
    memberCount,
    parentCollectionIds: []
  };
}

describe('optimistic collection creation', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetAllCollections.mockResolvedValue([]);
    collectionsData.reset();
  });

  it('shows the new collection in the tree before the backend create resolves', async () => {
    let resolveCreate: (id: string) => void = () => {};
    mockCreateCollection.mockReturnValue(
      new Promise<string>((resolve) => {
        resolveCreate = resolve;
      })
    );

    const createPromise = collectionsData.createCollection('Architecture');

    // Visible immediately — before the create call has resolved.
    const treeWhilePending = collectionsData.collectionsTree;
    expect(treeWhilePending.map((c) => c.name)).toEqual(['Architecture']);
    expect(treeWhilePending[0].memberCount).toBe(0);
    expect(treeWhilePending[0].pending).toBe(true);

    resolveCreate('real-id-1');
    await createPromise;
  });

  it('reconciles the temporary id with the backend id on success', async () => {
    mockCreateCollection.mockResolvedValue('real-id-1');

    const id = await collectionsData.createCollection('Architecture');

    expect(id).toBe('real-id-1');
    const tree = collectionsData.collectionsTree;
    expect(tree).toHaveLength(1);
    expect(tree[0].id).toBe('real-id-1');
    expect(tree[0].name).toBe('Architecture');
    // No longer pending once confirmed.
    expect(tree[0].pending).toBe(false);
  });

  it('keeps the confirmed collection visible even though it has zero members', async () => {
    mockCreateCollection.mockResolvedValue('real-id-1');
    await collectionsData.createCollection('Architecture');

    // A reload returns the real, still-empty collection. The hide-empty filter
    // must not swallow it — this is the regression the issue reported.
    mockGetAllCollections.mockResolvedValue([makeCollection('real-id-1', 'Architecture', 0)]);
    await collectionsData.loadCollections();

    const tree = collectionsData.collectionsTree;
    expect(tree.map((c) => c.id)).toEqual(['real-id-1']);
  });

  it('still hides empty collections the user did not create in this session', async () => {
    mockGetAllCollections.mockResolvedValue([
      makeCollection('other-empty', 'Someone Elses Empty', 0),
      makeCollection('populated', 'Populated', 3)
    ]);
    await collectionsData.loadCollections();

    expect(collectionsData.collectionsTree.map((c) => c.id)).toEqual(['populated']);
  });

  it('rolls the entry back out of the tree when the create fails', async () => {
    mockCreateCollection.mockRejectedValue(new Error('COLLECTION_EXISTS'));

    const id = await collectionsData.createCollection('Duplicate');

    expect(id).toBeNull();
    expect(collectionsData.collectionsTree).toEqual([]);
    expect(collectionsData.state.error).toContain('COLLECTION_EXISTS');
  });

  it('does not leave a rolled-back collection exempt from the hide-empty filter', async () => {
    mockCreateCollection.mockRejectedValue(new Error('boom'));
    await collectionsData.createCollection('Ghost');

    // The same name later arrives from the backend as a genuinely empty
    // collection; the failed create must not have left a lingering exemption.
    mockGetAllCollections.mockResolvedValue([makeCollection('ghost-id', 'Ghost', 0)]);
    await collectionsData.loadCollections();

    expect(collectionsData.collectionsTree).toEqual([]);
  });

  it('does not drop an unconfirmed entry when a reload races the create', async () => {
    let resolveCreate: (id: string) => void = () => {};
    mockCreateCollection.mockReturnValue(
      new Promise<string>((resolve) => {
        resolveCreate = resolve;
      })
    );

    const createPromise = collectionsData.createCollection('Architecture');

    // A refresh lands mid-flight and the backend does not know about the new
    // collection yet. The optimistic entry must survive it.
    mockGetAllCollections.mockResolvedValue([makeCollection('existing', 'Existing', 2)]);
    await collectionsData.loadCollections();

    expect(collectionsData.collectionsTree.map((c) => c.name)).toEqual(
      expect.arrayContaining(['Architecture', 'Existing'])
    );

    resolveCreate('real-id-1');
    await createPromise;

    const ids = collectionsData.collectionsTree.map((c) => c.id);
    expect(ids).toEqual(expect.arrayContaining(['real-id-1', 'existing']));
    // Exactly one entry for the new collection — no duplicate placeholder.
    expect(ids.filter((id) => id === 'real-id-1')).toHaveLength(1);
    expect(ids.some((id) => id.startsWith('pending-collection-'))).toBe(false);
  });

  it('drops the placeholder rather than duplicating when a reload already brought the real row in', async () => {
    let resolveCreate: (id: string) => void = () => {};
    mockCreateCollection.mockReturnValue(
      new Promise<string>((resolve) => {
        resolveCreate = resolve;
      })
    );

    const createPromise = collectionsData.createCollection('Architecture');

    // The reload sees the backend row before the create call returns its id.
    mockGetAllCollections.mockResolvedValue([makeCollection('real-id-1', 'Architecture', 0)]);
    await collectionsData.loadCollections();

    resolveCreate('real-id-1');
    await createPromise;

    const tree = collectionsData.collectionsTree;
    expect(tree.map((c) => c.id)).toEqual(['real-id-1']);
  });

  it('treats a falsy id from the backend as a failure rather than a confirmed create', async () => {
    // The browser dev proxy's unimplemented create resolves '' instead of
    // throwing. Reconciling that would seed a permanently-exempt collection
    // keyed on an empty id, which also collides in the sidebar's keyed each.
    mockCreateCollection.mockResolvedValue('');

    const id = await collectionsData.createCollection('Architecture');

    expect(id).toBeNull();
    expect(collectionsData.collectionsTree).toEqual([]);
    expect(collectionsData.state.collections).toEqual([]);
    expect(collectionsData.state.locallyCreatedIds.has('')).toBe(false);
    expect(collectionsData.state.error).toBeTruthy();
  });

  it('discards a create that resolves after a reset instead of leaking into the new store', async () => {
    let resolveCreate: (id: string) => void = () => {};
    mockCreateCollection.mockReturnValue(
      new Promise<string>((resolve) => {
        resolveCreate = resolve;
      })
    );

    const createPromise = collectionsData.createCollection('Architecture');

    collectionsData.reset();

    resolveCreate('real-id-1');
    // Reported as a failure: the collection is not in the store the caller can
    // see, so a non-null id would have the sidebar treat it as created+shown.
    await expect(createPromise).resolves.toBeNull();

    // The exemption belongs to the previous database. Collection ids are
    // derived from the name, so leaking it here could wrongly un-hide a
    // same-named empty collection in the newly-selected database.
    expect(collectionsData.state.locallyCreatedIds.size).toBe(0);
    expect(collectionsData.state.pendingIds.size).toBe(0);
    expect(collectionsData.state.collections).toEqual([]);

    mockGetAllCollections.mockResolvedValue([makeCollection('real-id-1', 'Architecture', 0)]);
    await collectionsData.loadCollections();
    expect(collectionsData.collectionsTree).toEqual([]);
  });

  it('does not leave a stale error on the store when a create fails after a reset', async () => {
    let rejectCreate: (err: Error) => void = () => {};
    mockCreateCollection.mockReturnValue(
      new Promise<string>((_resolve, reject) => {
        rejectCreate = reject;
      })
    );

    const createPromise = collectionsData.createCollection('Architecture');
    collectionsData.reset();

    rejectCreate(new Error('boom'));
    await createPromise;

    expect(collectionsData.state.error).toBeNull();
  });

  it('forgetLocallyCreated drops the exemptions carried into a different database', async () => {
    mockCreateCollection.mockResolvedValue('arch-id');
    await collectionsData.createCollection('Architecture');
    expect(collectionsData.state.locallyCreatedIds.has('arch-id')).toBe(true);

    // Switching databases: the exemption is per-database, because collection
    // ids are derived from the name — 'Architecture' has this same id in every
    // database, so carrying it over would un-hide an empty one there.
    collectionsData.forgetLocallyCreated();

    mockGetAllCollections.mockResolvedValue([makeCollection('arch-id', 'Architecture', 0)]);
    await collectionsData.loadCollections();

    expect(collectionsData.collectionsTree).toEqual([]);
  });

  it('discards a create that resolves after a database switch', async () => {
    let resolveCreate: (id: string) => void = () => {};
    mockCreateCollection.mockReturnValue(
      new Promise<string>((resolve) => {
        resolveCreate = resolve;
      })
    );

    const createPromise = collectionsData.createCollection('Architecture');
    collectionsData.forgetLocallyCreated();

    resolveCreate('arch-id');
    await expect(createPromise).resolves.toBeNull();

    expect(collectionsData.state.locallyCreatedIds.size).toBe(0);
    expect(collectionsData.state.pendingIds.size).toBe(0);
    // No error either — a discarded create is not a failure to report to the
    // user, so the sidebar leaves its form closed rather than showing a message
    // about a database they have already left.
    expect(collectionsData.state.error).toBeNull();
  });

  it('handles two concurrent creates without dropping or duplicating either', async () => {
    const resolvers: Array<(id: string) => void> = [];
    mockCreateCollection.mockImplementation(
      () =>
        new Promise<string>((resolve) => {
          resolvers.push(resolve);
        })
    );

    const first = collectionsData.createCollection('Alpha');
    const second = collectionsData.createCollection('Beta');

    expect(collectionsData.collectionsTree.map((c) => c.name)).toEqual(['Alpha', 'Beta']);

    // Resolve out of order to make sure each reconciles against its own temp id.
    resolvers[1]('id-beta');
    resolvers[0]('id-alpha');
    await Promise.all([first, second]);

    const ids = collectionsData.collectionsTree.map((c) => c.id);
    expect(ids).toEqual(expect.arrayContaining(['id-alpha', 'id-beta']));
    expect(ids).toHaveLength(2);
    expect(collectionsData.state.pendingIds.size).toBe(0);
    expect(ids.some((id) => id.startsWith('pending-collection-'))).toBe(false);
  });

  it('resets the locally-created exemption on store reset', async () => {
    mockCreateCollection.mockResolvedValue('real-id-1');
    await collectionsData.createCollection('Architecture');

    collectionsData.reset();

    mockGetAllCollections.mockResolvedValue([makeCollection('real-id-1', 'Architecture', 0)]);
    await collectionsData.loadCollections();

    // After a reset (e.g. database switch) the exemption is gone, so the empty
    // collection is filtered like any other.
    expect(collectionsData.collectionsTree).toEqual([]);
  });

  // Unlike every other cross-switch read in the codebase
  // (loadChildrenForParent, doLoadChildrenTree, createChat, createCollection),
  // loadCollections committed its fetched array into state without re-checking
  // the store generation after the await — a response issued against the
  // previous database could land after a switch and get committed as if it
  // belonged to the new one.
  describe('loadCollections stale-response guard across a database switch', () => {
    it('discards a load that resolves after forgetLocallyCreated, without writing into the list', async () => {
      let resolveLoad: (cols: CollectionInfo[]) => void = () => {};
      mockGetAllCollections.mockReturnValue(
        new Promise<CollectionInfo[]>((resolve) => {
          resolveLoad = resolve;
        })
      );

      const loadPromise = collectionsData.loadCollections();

      // A database switch lands while the load is still in flight.
      collectionsData.forgetLocallyCreated();

      resolveLoad([makeCollection('stale-db-col', 'Stale', 3)]);
      await loadPromise;

      expect(collectionsData.state.collections).toEqual([]);
    });

    it('discards a load that resolves after reset, without writing into the list', async () => {
      let resolveLoad: (cols: CollectionInfo[]) => void = () => {};
      mockGetAllCollections.mockReturnValue(
        new Promise<CollectionInfo[]>((resolve) => {
          resolveLoad = resolve;
        })
      );

      const loadPromise = collectionsData.loadCollections();
      collectionsData.reset();

      resolveLoad([makeCollection('stale-db-col', 'Stale', 3)]);
      await loadPromise;

      expect(collectionsData.state.collections).toEqual([]);
    });

    it('a late-resolving load from the previous database does not clobber a fresh load for the new one', async () => {
      // Reproduces the exact failure scenario: DB A's loadCollections is still
      // in flight when the user switches to DB B. B's fresh load resolves
      // first; A's late response must not then overwrite it.
      let resolveFirst: (cols: CollectionInfo[]) => void = () => {};
      mockGetAllCollections.mockReturnValueOnce(
        new Promise<CollectionInfo[]>((resolve) => {
          resolveFirst = resolve;
        })
      );

      const firstLoad = collectionsData.loadCollections(); // issued against DB A

      collectionsData.forgetLocallyCreated(); // switch to DB B
      mockGetAllCollections.mockResolvedValueOnce([makeCollection('b-col', 'From DB B', 2)]);
      const secondLoad = collectionsData.loadCollections(); // issued against DB B
      await secondLoad;

      expect(collectionsData.state.collections.map((c) => c.id)).toEqual(['b-col']);

      // DB A's stale response finally lands.
      resolveFirst([makeCollection('a-col', 'From DB A', 3)]);
      await firstLoad;

      // Still DB B's data — the stale DB A response was dropped, not merged or
      // committed over it.
      expect(collectionsData.state.collections.map((c) => c.id)).toEqual(['b-col']);
    });

    it('discards a load failure that resolves after forgetLocallyCreated, without surfacing a stale error', async () => {
      let rejectLoad: (err: Error) => void = () => {};
      mockGetAllCollections.mockReturnValue(
        new Promise<CollectionInfo[]>((_resolve, reject) => {
          rejectLoad = reject;
        })
      );

      const loadPromise = collectionsData.loadCollections();
      collectionsData.forgetLocallyCreated();
      rejectLoad(new Error('boom in old database'));

      await loadPromise;

      expect(collectionsData.state.error).toBeNull();
    });
  });
});
