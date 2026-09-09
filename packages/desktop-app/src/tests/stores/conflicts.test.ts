/**
 * Conflicts store (ADR-068): `load`, `hasOpenFor`, and the resolution
 * actions. Unlike the deleted `recovered-items.svelte.ts`, this store is
 * NOT Pro-gated — the conflict journal is designed to work on a purely
 * local-only install (that's the defect ADR-068 fixes), so `load()` always
 * calls the daemon regardless of `proSync.isPro`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

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

import { conflictsStore, type ConflictRecord } from '$lib/stores/conflicts.svelte';
import { backendAdapter } from '$lib/services/backend-adapter';
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

      await expect(conflictsStore.load()).resolves.toBeUndefined();

      expect(conflictsStore.records).toEqual([]);
      expect(conflictsStore.loaded).toBe(true);
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

    it('throws without calling resolve_conflict when the node no longer exists', async () => {
      vi.spyOn(backendAdapter, 'getNode').mockResolvedValueOnce(null);

      await expect(conflictsStore.rename('c1', 'gone', 'Work', 'Work (EU)')).rejects.toThrow();

      expect(mockInvoke).not.toHaveBeenCalled();
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
