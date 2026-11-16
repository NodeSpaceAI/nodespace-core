/**
 * Moving a collection under another, taking it out of a parent and renaming it,
 * through the collections store: what is called on the backend, what is refused
 * before any call, and the message each failure comes back with.
 */
import { describe, it, expect, beforeEach, afterEach, vi, type MockInstance } from 'vitest';
import { collectionsData, collectionsState } from '$lib/stores/collections.svelte';
import { collectionService, type CollectionInfo } from '$lib/services/collection-service';

function collection(
  id: string,
  name: string,
  parentCollectionIds: string[] = [],
  version = 1
): CollectionInfo {
  return {
    lifecycleStatus: 'active',
    id,
    content: name,
    nodeType: 'collection',
    createdAt: '',
    modifiedAt: '',
    version,
    properties: {},
    memberCount: 1,
    parentCollectionIds
  };
}

const COLLECTIONS = [
  collection('deutnet', 'DeutNet'),
  collection('company-policies', 'Company Policies', ['deutnet']),
  collection('hr', 'HR', ['company-policies'], 7),
  collection('design', 'Design')
];

describe('collectionsData — moving and renaming collections', () => {
  let add: MockInstance<typeof collectionService.addNodeToCollection>;
  let remove: MockInstance<typeof collectionService.removeNodeFromCollection>;
  let rename: MockInstance<typeof collectionService.renameCollection>;
  let load: MockInstance<typeof collectionService.getAllCollections>;

  beforeEach(() => {
    collectionsData._setTestData(COLLECTIONS, new Map());
    add = vi.spyOn(collectionService, 'addNodeToCollection').mockResolvedValue();
    remove = vi.spyOn(collectionService, 'removeNodeFromCollection').mockResolvedValue();
    rename = vi
      .spyOn(collectionService, 'renameCollection')
      .mockResolvedValue({} as Awaited<ReturnType<typeof collectionService.renameCollection>>);
    load = vi.spyOn(collectionService, 'getAllCollections').mockResolvedValue(COLLECTIONS);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    collectionsData.reset();
    collectionsState.reset();
  });

  describe('moveCollectionInto', () => {
    it('adds the parent on the backend and reloads the list', async () => {
      const result = await collectionsData.moveCollectionInto('design', 'deutnet');

      expect(result).toEqual({ ok: true });
      expect(add).toHaveBeenCalledExactlyOnceWith('design', 'deutnet');
      expect(load).toHaveBeenCalledTimes(1);
    });

    it('refuses a cycle before any call, naming both collections', async () => {
      const result = await collectionsData.moveCollectionInto('deutnet', 'company-policies');

      expect(result).toEqual({
        ok: false,
        message:
          '“DeutNet” can\'t go into “Company Policies” because “Company Policies” is already inside “DeutNet”.'
      });
      expect(add).not.toHaveBeenCalled();
      expect(load).not.toHaveBeenCalled();
    });

    it('refuses a move into itself and into a parent it already has', async () => {
      expect(await collectionsData.moveCollectionInto('design', 'design')).toMatchObject({
        ok: false
      });
      expect(await collectionsData.moveCollectionInto('hr', 'company-policies')).toEqual({
        ok: false,
        message: '“HR” is already in “Company Policies”.'
      });
      expect(add).not.toHaveBeenCalled();
    });

    it("gives the backend's reason when the backend refuses, and does not reload", async () => {
      add.mockRejectedValueOnce({
        message: 'Failed to add node to collection: You do not have access to “DeutNet”',
        code: 'COLLECTION_ERROR'
      });

      const result = await collectionsData.moveCollectionInto('design', 'deutnet');

      expect(result).toEqual({
        ok: false,
        message: 'Couldn\'t move “Design” into “DeutNet”: You do not have access to “DeutNet”'
      });
      expect(load).not.toHaveBeenCalled();
    });

    it('reads the backend cycle refusal as the same cycle message', async () => {
      // The list the store holds is stale (it does not yet know the cycle), so the
      // backend is what catches it.
      collectionsData._setTestData(
        [collection('a', 'A'), collection('b', 'B')],
        new Map()
      );
      add.mockRejectedValueOnce({
        message: 'Failed to add node to collection: collection_cycle: x',
        code: 'COLLECTION_ERROR'
      });

      const result = await collectionsData.moveCollectionInto('a', 'b');

      expect(result).toEqual({
        ok: false,
        message: '“A” can\'t go into “B” because “B” is already inside “A”.'
      });
    });

    it('says so when either collection is gone from the list', async () => {
      const result = await collectionsData.moveCollectionInto('design', 'missing');

      expect(result.ok).toBe(false);
      expect(add).not.toHaveBeenCalled();
    });

    it('reports a switched database and does not reload into the new one', async () => {
      add.mockImplementationOnce(async () => {
        collectionsData.reset();
      });

      const result = await collectionsData.moveCollectionInto('design', 'deutnet');

      expect(result).toEqual({
        ok: false,
        message: 'Nothing was changed: the database was switched.'
      });
      expect(load).not.toHaveBeenCalled();
    });
  });

  describe('removeCollectionFrom', () => {
    it('takes the collection out of that one parent and reloads', async () => {
      const result = await collectionsData.removeCollectionFrom('company-policies', 'deutnet');

      expect(result).toEqual({ ok: true });
      expect(remove).toHaveBeenCalledExactlyOnceWith('company-policies', 'deutnet');
      expect(load).toHaveBeenCalledTimes(1);
    });

    it('names the parent when the backend refuses', async () => {
      remove.mockRejectedValueOnce({ message: 'Failed to remove node from collection: nope' });

      expect(await collectionsData.removeCollectionFrom('company-policies', 'deutnet')).toEqual({
        ok: false,
        message: 'Couldn\'t remove “Company Policies” from “DeutNet”: nope'
      });
      expect(load).not.toHaveBeenCalled();
    });
  });

  describe('renameCollection', () => {
    it("renames with the collection's current version and reloads", async () => {
      const result = await collectionsData.renameCollection('hr', '  People  ');

      expect(result).toEqual({ ok: true });
      expect(rename).toHaveBeenCalledExactlyOnceWith('hr', 7, 'People');
      expect(load).toHaveBeenCalledTimes(1);
    });

    it('refuses an empty name without a call', async () => {
      expect(await collectionsData.renameCollection('hr', '   ')).toEqual({
        ok: false,
        message: 'A collection needs a name.'
      });
      expect(rename).not.toHaveBeenCalled();
    });

    it('does nothing for the name it already has', async () => {
      expect(await collectionsData.renameCollection('hr', 'HR')).toEqual({ ok: true });
      expect(rename).not.toHaveBeenCalled();
      expect(load).not.toHaveBeenCalled();
    });

    it('refuses a name another collection has, ignoring letter case, without a call', async () => {
      expect(await collectionsData.renameCollection('hr', 'design')).toEqual({
        ok: false,
        message: 'A collection named “Design” already exists.'
      });
      expect(rename).not.toHaveBeenCalled();
    });

    it('turns the backend refusals into messages', async () => {
      rename.mockRejectedValueOnce({ message: 'x', code: 'COLLECTION_EXISTS' });
      expect(await collectionsData.renameCollection('hr', 'People')).toEqual({
        ok: false,
        message: 'A collection named “People” already exists.'
      });

      rename.mockRejectedValueOnce({
        message: 'Failed to rename collection: Version conflict on hr: expected 7, got 8',
        code: 'UPDATE_ERROR',
        details: 'Aborted'
      });
      expect(await collectionsData.renameCollection('hr', 'People')).toEqual({
        ok: false,
        message:
          '“HR” was changed somewhere else. The list is up to date now: try renaming it again.'
      });
    });

    it('refreshes the list after a failed rename, so the retry goes out with the current version', async () => {
      rename.mockRejectedValueOnce({
        message: 'Failed to rename collection: Version conflict on hr: expected 7, got 8',
        code: 'UPDATE_ERROR',
        details: 'Aborted'
      });
      load.mockResolvedValue(COLLECTIONS.map((c) => (c.id === 'hr' ? { ...c, version: 8 } : c)));

      const first = await collectionsData.renameCollection('hr', 'People');
      const retry = await collectionsData.renameCollection('hr', 'People');

      expect(first.ok).toBe(false);
      expect(load).toHaveBeenCalledTimes(2); // after the failure, and after the retry succeeded
      expect(retry).toEqual({ ok: true });
      expect(rename).toHaveBeenNthCalledWith(1, 'hr', 7, 'People');
      expect(rename).toHaveBeenNthCalledWith(2, 'hr', 8, 'People');
    });

    it('does not rename a collection whose create has not finished', async () => {
      collectionsData._setTestData([collection('pending-collection-1', 'New')], new Map());

      const result = await collectionsData.renameCollection('pending-collection-1', 'Newer');

      expect(result.ok).toBe(false);
      expect(rename).not.toHaveBeenCalled();
    });
  });

  describe('moveTargetsFor and parentsOf', () => {
    it('list where a collection can go and what it can be removed from', () => {
      expect(collectionsData.moveTargetsFor('design').map((c) => c.id)).toEqual([
        'company-policies',
        'deutnet',
        'hr'
      ]);
      expect(collectionsData.parentsOf('hr')).toEqual([
        { id: 'company-policies', name: 'Company Policies' }
      ]);
      expect(collectionsData.parentsOf('design')).toEqual([]);
    });
  });

  describe('where the tree lists a collection', () => {
    it('lists a collection under each unrelated parent, and not under an ancestor of one of them', () => {
      collectionsData._setTestData(
        [
          collection('top', 'Top'),
          collection('mid', 'Mid', ['top']),
          collection('other', 'Other'),
          collection('leaf', 'Leaf', ['top', 'mid', 'other'])
        ],
        new Map()
      );

      expect(collectionsData.shownUnderIds('leaf').sort()).toEqual(['mid', 'other']);
      expect(collectionsData.isShownUnder('leaf', 'mid')).toBe(true);
      expect(collectionsData.isShownUnder('leaf', 'top')).toBe(false);
      expect(collectionsData.shownUnderNames('leaf').sort()).toEqual(['Mid', 'Other']);
    });

    it('says whether a collection can be moved anywhere', () => {
      expect(collectionsData.canMove('design')).toBe(true);
      collectionsData._setTestData([collection('only', 'Only')], new Map());
      expect(collectionsData.canMove('only')).toBe(false);
    });
  });

  describe('expandCollection', () => {
    it('opens a collection in the tree and leaves an open one open', () => {
      collectionsState.expandCollection('deutnet');
      collectionsState.expandCollection('deutnet');

      expect(collectionsState.state.expandedCollectionIds.has('deutnet')).toBe(true);
    });
  });
});
