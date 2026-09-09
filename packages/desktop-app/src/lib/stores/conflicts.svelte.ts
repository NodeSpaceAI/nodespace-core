import { invoke } from '@tauri-apps/api/core';
import { createLogger } from '$lib/utils/logger';

const log = createLogger('Conflicts');

export type ConflictKind =
  | 'unique_field_collision'
  | 'collection_name_collision'
  | 'superseded_edit'
  | 'duplicate_reactive_create';

export type ConflictStatus = 'open' | 'resolved' | 'dismissed';

/** One conflict-journal record (ADR-068). `detail`/`resolution` are already
 * parsed JSON objects by the time they reach the frontend — see
 * `commands::conflicts::proto_to_conflict_record` on the Tauri side. */
export interface ConflictRecord {
  id: string;
  kind: ConflictKind;
  nodeIds: string[];
  detail: Record<string, unknown>;
  status: ConflictStatus;
  detectedAt: string;
  detectedBy: string | null;
  occurrences: number;
  lastSeenAt: string;
  resolvedAt: string | null;
  resolution: Record<string, unknown> | null;
}

/** One entry of the JSON-encoded `Resolution` shape the backend expects. */
export type Resolution =
  | { action: 'dismiss' }
  | { action: 'adopt_existing'; adopted: string }
  | { action: 'rename'; renamed: string; from: string; to: string }
  | { action: 'restore'; restored_to: string }
  | {
      action: 'merge';
      survivor: string;
      loser: string;
      superseded: Record<string, unknown>;
      edges_repointed: number;
      edges_dropped: number;
    };

/**
 * The Conflicts store (ADR-068, conflict-journal-and-resolution.md §6.4):
 * `records` + a `hasOpenFor` derived lookup for the inline per-node
 * indicator, plus the resolution actions. Follows the canonical rune
 * pattern (ADR-049) the deleted `recovered-items.svelte.ts` store
 * established: `$state` fields directly on a plain class, a `$derived` used
 * only to memoize a lookup, methods that read state directly — no `$effect`.
 */
class ConflictsStore {
  records = $state<ConflictRecord[]>([]);
  loaded = $state(false);

  private openNodeIds = $derived(
    new Set(
      this.records.filter((r) => r.status === 'open').flatMap((r) => r.nodeIds)
    )
  );

  /** Does `nodeId` participate in at least one OPEN conflict? Drives the
   * inline indicator — a derived read of the journal, never a stored
   * property (conflict-journal-and-resolution.md §6.3). */
  hasOpenFor(nodeId: string): boolean {
    return this.openNodeIds.has(nodeId);
  }

  /** Load every conflict record (used by the Conflicts view). */
  async load(): Promise<void> {
    try {
      const records = await invoke<ConflictRecord[]>('list_conflicts', {
        input: { status: null, kind: null, limit: null }
      });
      this.records = records ?? [];
    } catch (e) {
      log.warn('Failed to load conflicts', { error: e });
      this.records = [];
    } finally {
      this.loaded = true;
    }
  }

  /** Load only the records naming `nodeId` — used by the inline indicator so
   * it doesn't have to wait on (or trigger) a full-list load. */
  async loadForNode(nodeId: string): Promise<ConflictRecord[]> {
    try {
      const records = await invoke<ConflictRecord[]>('conflicts_for_node', { nodeId });
      // Merge into the shared cache by id so the full Conflicts view (if open
      // in another pane) and this per-node lookup never disagree.
      const byId = new Map(this.records.map((r) => [r.id, r]));
      for (const r of records ?? []) byId.set(r.id, r);
      this.records = Array.from(byId.values());
      return records ?? [];
    } catch (e) {
      log.warn('Failed to load conflicts for node', { error: e, nodeId });
      return [];
    }
  }

  private async resolve(conflictId: string, resolution: Resolution): Promise<void> {
    try {
      const updated = await invoke<ConflictRecord>('resolve_conflict', {
        conflictId,
        resolution
      });
      this.records = this.records.map((r) => (r.id === conflictId ? updated : r));
    } catch (e) {
      log.warn('Failed to resolve conflict', { error: e, conflictId, resolution });
      throw e;
    }
  }

  /** Dismiss a conflict record — persisted; re-detection will not re-raise it. */
  async dismiss(conflictId: string): Promise<void> {
    await this.resolve(conflictId, { action: 'dismiss' });
  }

  /** Non-destructive: record that the existing node was adopted instead of a
   * newly-created duplicate. */
  async adoptExisting(conflictId: string, adopted: string): Promise<void> {
    await this.resolve(conflictId, { action: 'adopt_existing', adopted });
  }

  /**
   * Merge `loserId` into `survivorId` (ADR-068 §5.2) — property union, edge
   * re-pointing, the loser archived. **User-initiated only**: call this
   * exclusively from an explicit user action in the Conflicts view, never
   * automatically at any confidence level (a shared value is evidence, not
   * proof — an auto-merge on a false positive would silently destroy a
   * distinct node's data and re-point its edges onto the wrong survivor).
   *
   * If `conflictId` is given, the record is closed as `resolved` with a
   * `Resolution::Merge` server-side in the same transaction; this refreshes
   * that record locally afterward so the UI reflects it without a full reload.
   */
  async merge(
    survivorId: string,
    loserId: string,
    conflictId?: string
  ): Promise<MergeOutcome> {
    const outcome = await invoke<MergeOutcome>('merge_nodes', {
      survivorId,
      loserId,
      conflictId: conflictId ?? null
    });
    if (conflictId) {
      await this.loadForNode(survivorId);
    }
    return outcome;
  }
}

export interface MergeOutcome {
  survivorId: string;
  loserId: string;
  propertiesMerged: number;
  edgesRepointed: number;
  edgesDropped: number;
}

export const conflictsStore = new ConflictsStore();
