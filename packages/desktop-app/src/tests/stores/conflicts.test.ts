/**
 * Conflicts store (ADR-068): `load`, `hasOpenFor`, and the resolution
 * actions. The conflict journal records conflicts a local install detects
 * itself, so `load()` always calls the daemon. Also covers the two conflict
 * kinds end to end: the store's kind union and the Conflicts view's handling
 * of each.
 */
import { describe, it, expect, expectTypeOf, beforeEach, afterEach, vi } from 'vitest';
import { render, cleanup } from '@testing-library/svelte';

const mockInvoke = vi.fn();
import { mockTauriCore } from '../helpers/mock-tauri-core';

vi.mock('@tauri-apps/api/core', () =>
  mockTauriCore({ invoke: (...args: unknown[]) => mockInvoke(...args) })
);

vi.mock('$lib/utils/logger', () => ({
  createLogger: () => ({
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  })
}));

vi.mock('$lib/services/navigation-service', () => ({
  getNavigationService: () => ({ focusOrOpenNode: vi.fn() })
}));

import ConflictsPane from '$lib/components/conflicts/conflicts-pane.svelte';
import {
  conflictsStore,
  type ConflictKind,
  type ConflictRecord
} from '$lib/stores/conflicts.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
import * as collectionRefresh from '$lib/utils/collection-refresh';
import type { Node } from '$lib/types';

function record(overrides: Partial<ConflictRecord> = {}): ConflictRecord {
  return {
    id: 'c1',
    kind: 'unique_field_collision',
    nodeIds: ['n1', 'n2'],
    detail: { node_type: 'person', field: 'email', value: 'a@example.com' },
    status: 'open',
    detectedAt: '2026-01-01T00:00:00Z',
    detectedBy: null,
    occurrences: 1,
    lastSeenAt: '2026-01-01T00:00:00Z',
    resolvedAt: null,
    resolution: null,
    ...overrides
  };
}

describe('conflicts store', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    conflictsStore.records = [];
    conflictsStore.loaded = false;
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('load()', () => {
    it('populates records from list_conflicts', async () => {
      mockInvoke.mockResolvedValueOnce([record()]);

      await conflictsStore.load();

      expect(mockInvoke).toHaveBeenCalledWith('list_conflicts', {
        input: { status: null, kind: null, limit: null }
      });
      expect(conflictsStore.records).toEqual([record()]);
      expect(conflictsStore.loaded).toBe(true);
    });

    it('clears records and marks loaded on failure, without throwing', async () => {
      mockInvoke.mockRejectedValueOnce(new Error('daemon unreachable'));

      await expect(conflictsStore.load()).resolves.toBe(true);

      expect(conflictsStore.records).toEqual([]);
      expect(conflictsStore.loaded).toBe(true);
    });
  });

  describe('invalidateForDatabaseSwitch()', () => {
    it('clears the previous database records immediately', () => {
      conflictsStore.records = [record()];
      conflictsStore.loaded = true;

      conflictsStore.invalidateForDatabaseSwitch();

      expect(conflictsStore.records).toEqual([]);
      expect(conflictsStore.loaded).toBe(false);
      expect(conflictsStore.hasOpenFor('n1')).toBe(false);
    });

    it('drops a load issued before the switch that resolves after the reload', async () => {
      let resolveStale!: (v: ConflictRecord[]) => void;
      mockInvoke.mockImplementationOnce(
        () => new Promise<ConflictRecord[]>((r) => (resolveStale = r))
      );
      const stale = conflictsStore.load();

      conflictsStore.invalidateForDatabaseSwitch();
      mockInvoke.mockResolvedValueOnce([record({ id: 'new-db', nodeIds: ['m1'] })]);
      await expect(conflictsStore.load()).resolves.toBe(true);

      resolveStale([record({ id: 'old-db', nodeIds: ['n1'] })]);
      await expect(stale).resolves.toBe(false);

      expect(conflictsStore.records.map((r) => r.id)).toEqual(['new-db']);
      expect(conflictsStore.hasOpenFor('n1')).toBe(false);
    });

    it('drops a failed load issued before the switch instead of clearing the new records', async () => {
      let rejectStale!: (e: Error) => void;
      mockInvoke.mockImplementationOnce(
        () => new Promise<ConflictRecord[]>((_, rej) => (rejectStale = rej))
      );
      const stale = conflictsStore.load();

      conflictsStore.invalidateForDatabaseSwitch();
      mockInvoke.mockResolvedValueOnce([record({ id: 'new-db' })]);
      await conflictsStore.load();

      rejectStale(new Error('daemon unreachable'));
      await expect(stale).resolves.toBe(false);

      expect(conflictsStore.records.map((r) => r.id)).toEqual(['new-db']);
    });

    it('drops a per-node load issued before the switch', async () => {
      let resolveStale!: (v: ConflictRecord[]) => void;
      mockInvoke.mockImplementationOnce(
        () => new Promise<ConflictRecord[]>((r) => (resolveStale = r))
      );
      const stale = conflictsStore.loadForNode('n1');

      conflictsStore.invalidateForDatabaseSwitch();
      resolveStale([record({ id: 'old-db', nodeIds: ['n1'] })]);

      await expect(stale).resolves.toEqual([]);
      expect(conflictsStore.records).toEqual([]);
    });

    it('does not patch a same-id record in the new database with a pre-switch resolution', async () => {
      let resolveStale!: (v: ConflictRecord) => void;
      mockInvoke.mockImplementationOnce(
        () => new Promise<ConflictRecord>((r) => (resolveStale = r))
      );
      const stale = conflictsStore.dismiss('c1');

      conflictsStore.invalidateForDatabaseSwitch();
      conflictsStore.records = [record({ id: 'c1' })];
      resolveStale(record({ id: 'c1', status: 'dismissed' }));
      await stale;

      expect(conflictsStore.records[0].status).toBe('open');
    });

    it('skips the post-merge refresh when a switch landed during the merge', async () => {
      let resolveMerge!: (v: unknown) => void;
      mockInvoke.mockImplementationOnce(() => new Promise((r) => (resolveMerge = r)));
      const merging = conflictsStore.merge('n1', 'n2', 'c1');

      conflictsStore.invalidateForDatabaseSwitch();
      resolveMerge({
        survivorId: 'n1',
        loserId: 'n2',
        propertiesMerged: 0,
        edgesRepointed: 0,
        edgesDropped: 0
      });
      await merging;

      expect(mockInvoke).toHaveBeenCalledTimes(1);
      expect(mockInvoke).not.toHaveBeenCalledWith('conflicts_for_node', expect.anything());
    });
  });

  describe('hasOpenFor()', () => {
    it('is true for a node named by an OPEN record', () => {
      conflictsStore.records = [record({ nodeIds: ['n1', 'n2'], status: 'open' })];
      expect(conflictsStore.hasOpenFor('n1')).toBe(true);
      expect(conflictsStore.hasOpenFor('n2')).toBe(true);
    });

    it('is false for a node named only by a DISMISSED record', () => {
      conflictsStore.records = [record({ nodeIds: ['n1'], status: 'dismissed' })];
      expect(conflictsStore.hasOpenFor('n1')).toBe(false);
    });

    it('is false for a node named by no record at all', () => {
      conflictsStore.records = [record({ nodeIds: ['n1'] })];
      expect(conflictsStore.hasOpenFor('unrelated')).toBe(false);
    });
  });

  describe('loadForNode()', () => {
    it('merges results into the shared cache by id', async () => {
      conflictsStore.records = [record({ id: 'existing' })];
      mockInvoke.mockResolvedValueOnce([record({ id: 'c2', nodeIds: ['n3'] })]);

      const result = await conflictsStore.loadForNode('n3');

      expect(mockInvoke).toHaveBeenCalledWith('conflicts_for_node', { nodeId: 'n3' });
      expect(result).toEqual([record({ id: 'c2', nodeIds: ['n3'] })]);
      expect(conflictsStore.records.map((r) => r.id).sort()).toEqual(['c2', 'existing']);
    });
  });

  describe('dismiss()', () => {
    it('calls resolve_conflict with a dismiss resolution and updates the record in place', async () => {
      conflictsStore.records = [record({ id: 'c1', status: 'open' })];
      mockInvoke.mockResolvedValueOnce(record({ id: 'c1', status: 'dismissed' }));

      await conflictsStore.dismiss('c1');

      expect(mockInvoke).toHaveBeenCalledWith('resolve_conflict', {
        conflictId: 'c1',
        resolution: { action: 'dismiss' }
      });
      expect(conflictsStore.records[0].status).toBe('dismissed');
    });
  });

  describe('rename()', () => {
    it('updates the node then records a rename resolution', async () => {
      const existing: Partial<Node> = { id: 'coll-1', version: 3 };
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValueOnce(existing as Node);
      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValueOnce({} as Node);
      vi.spyOn(sharedNodeStore, 'setNode').mockReturnValue(true);
      vi.spyOn(collectionRefresh, 'scheduleCollectionRefresh').mockImplementation(() => {});
      conflictsStore.records = [
        record({ id: 'c1', status: 'open', nodeIds: ['coll-1', 'coll-2'] })
      ];
      mockInvoke.mockResolvedValueOnce(record({ id: 'c1', status: 'resolved' }));

      await conflictsStore.rename('c1', 'coll-1', 'Work', 'Work (EU)');

      expect(backendAdapter.getNode).toHaveBeenCalledWith('coll-1');
      expect(backendAdapter.updateNode).toHaveBeenCalledWith('coll-1', 3, {
        content: 'Work (EU)'
      });
      expect(mockInvoke).toHaveBeenCalledWith('resolve_conflict', {
        conflictId: 'c1',
        resolution: { action: 'rename', renamed: 'coll-1', from: 'Work', to: 'Work (EU)' }
      });
      expect(conflictsStore.records[0].status).toBe('resolved');
    });

    it('applies the renamed node to the store and refreshes the collections sidebar', async () => {
      const renamed = { id: 'coll-1', content: 'Work (EU)', version: 4 } as Node;
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValueOnce({ id: 'coll-1', version: 3 } as Node);
      vi.spyOn(backendAdapter, 'updateNode').mockResolvedValueOnce(renamed);
      const setNode = vi.spyOn(sharedNodeStore, 'setNode').mockReturnValue(true);
      const refresh = vi
        .spyOn(collectionRefresh, 'scheduleCollectionRefresh')
        .mockImplementation(() => {});
      mockInvoke.mockResolvedValueOnce(record({ id: 'c1', status: 'resolved' }));

      await conflictsStore.rename('c1', 'coll-1', 'Work', 'Work (EU)');

      expect(setNode).toHaveBeenCalledWith(renamed, expect.objectContaining({ type: 'database' }), true);
      expect(refresh).toHaveBeenCalled();
    });

    it('throws without calling resolve_conflict when the node no longer exists', async () => {
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValueOnce(null);

      await expect(conflictsStore.rename('c1', 'gone', 'Work', 'Work (EU)')).rejects.toThrow();

      expect(mockInvoke).not.toHaveBeenCalled();
    });
  });

  describe('previewMerge()', () => {
    it('calls preview_merge and returns where the survivor ends up', async () => {
      const preview = { survivorParentId: null, loserParentId: 'p', resultingParentId: 'p' };
      mockInvoke.mockResolvedValueOnce(preview);

      const result = await conflictsStore.previewMerge('survivor', 'loser');

      expect(mockInvoke).toHaveBeenCalledWith('preview_merge', {
        survivorId: 'survivor',
        loserId: 'loser'
      });
      expect(result).toEqual(preview);
    });

    it('rejects with the typed tree-invariant refusal, unchanged', async () => {
      const refusal = {
        code: 'TREE_INVARIANT_VIOLATION',
        message: 'member_of_not_root: refused',
        conflictData: {
          rule: 'member_of_not_root',
          node_id: 'survivor',
          related_ids: ['col'],
          detail: 'refused'
        }
      };
      mockInvoke.mockRejectedValueOnce(refusal);

      await expect(conflictsStore.previewMerge('survivor', 'loser')).rejects.toBe(refusal);
    });
  });

  describe('merge()', () => {
    it('calls merge_nodes with the survivor/loser/conflictId and refreshes the record', async () => {
      conflictsStore.records = [record({ id: 'c1', status: 'open', nodeIds: ['survivor', 'loser'] })];
      const outcome = {
        survivorId: 'survivor',
        loserId: 'loser',
        propertiesMerged: 1,
        edgesRepointed: 2,
        edgesDropped: 0
      };
      mockInvoke.mockResolvedValueOnce(outcome); // merge_nodes
      mockInvoke.mockResolvedValueOnce([record({ id: 'c1', status: 'resolved' })]); // conflicts_for_node refresh

      const result = await conflictsStore.merge('survivor', 'loser', 'c1');

      expect(mockInvoke).toHaveBeenNthCalledWith(1, 'merge_nodes', {
        survivorId: 'survivor',
        loserId: 'loser',
        conflictId: 'c1'
      });
      expect(mockInvoke).toHaveBeenNthCalledWith(2, 'conflicts_for_node', { nodeId: 'survivor' });
      expect(result).toEqual(outcome);
      expect(conflictsStore.records.find((r) => r.id === 'c1')?.status).toBe('resolved');
    });

    it('does not refresh when no conflictId is given (a standalone merge, no journal entry)', async () => {
      const outcome = {
        survivorId: 'survivor',
        loserId: 'loser',
        propertiesMerged: 0,
        edgesRepointed: 0,
        edgesDropped: 0
      };
      mockInvoke.mockResolvedValueOnce(outcome);

      await conflictsStore.merge('survivor', 'loser');

      expect(mockInvoke).toHaveBeenCalledTimes(1);
      expect(mockInvoke).toHaveBeenCalledWith('merge_nodes', {
        survivorId: 'survivor',
        loserId: 'loser',
        conflictId: null
      });
    });
  });
});

describe('conflict kinds', () => {
  const LABELS: Record<string, string> = {
    a1: 'Alice',
    a2: 'Alice (dup)',
    p1: 'People',
    p2: 'People (dup)'
  };

  beforeEach(() => {
    mockInvoke.mockReset();
    conflictsStore.records = [];
    conflictsStore.loaded = false;
    vi.spyOn(backendAdapter, 'getNode').mockImplementation(
      async (id: string) => ({ id, content: LABELS[id] ?? id, nodeType: 'person' }) as Node
    );
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('are exactly the two kinds a local install detects', () => {
    expectTypeOf<ConflictKind>().toEqualTypeOf<
      'unique_field_collision' | 'collection_name_collision'
    >();
  });

  it('each get a labelled group in the Conflicts view, with merge and adopt actions', async () => {
    const records: ConflictRecord[] = [
      record({ id: 'field', nodeIds: ['a1', 'a2'], detectedAt: '2026-01-02T00:00:00Z' }),
      record({
        id: 'name',
        kind: 'collection_name_collision',
        nodeIds: ['p1', 'p2'],
        detail: { name: 'people' }
      })
    ];
    mockInvoke.mockImplementation(async (cmd: string) =>
      cmd === 'list_conflicts' ? records : []
    );

    const view = render(ConflictsPane);

    const headings = await view.findAllByRole('heading', { level: 2 });
    expect(headings.map((h) => h.textContent)).toEqual([
      'Duplicate field value',
      'Duplicate collection name'
    ]);
    expect(await view.findByRole('button', { name: 'Keep Alice' })).toBeTruthy();
    expect(await view.findByRole('button', { name: 'Keep People' })).toBeTruthy();
    expect(view.getAllByRole('button', { name: 'Adopt existing' })).toHaveLength(2);
    // Rename is offered for a collection-name collision only.
    expect(view.getAllByRole('button', { name: 'Rename' })).toHaveLength(1);
  });
});
