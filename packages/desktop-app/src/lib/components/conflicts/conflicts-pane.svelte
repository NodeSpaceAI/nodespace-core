<!--
  Conflicts view (ADR-068, conflict-journal-and-resolution.md §6.2): a flat
  list of conflict records, grouped by kind, replacing the half-places that
  existed before this ADR (the possible-duplicate badge and the
  conflict-notifications toast cap).

  Open records are shown by default; resolved/dismissed are reachable behind
  a filter toggle, never deleted — the record of what a user decided is the
  point (§6.2). Empty state reads as healthy, not as an error.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    conflictsStore,
    type ConflictRecord,
    type ConflictKind,
    type MergePreview
  } from '$lib/stores/conflicts.svelte';
  import { isTreeInvariantViolation } from '$lib/types/errors';
  import {
    describeMergeRefusal,
    describeSurvivorPosition,
    type LabelOf
  } from './merge-messages';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('ConflictsPane');

  let showResolved = $state(false);
  let participantLabels = $state<Map<string, string>>(new Map());
  let participantTypes = $state<Map<string, string>>(new Map());

  const KIND_LABELS: Record<ConflictKind, string> = {
    unique_field_collision: 'Duplicate field value',
    collection_name_collision: 'Duplicate collection name'
  };

  onMount(() => {
    void conflictsStore.load();
  });

  const visibleRecords = $derived(
    conflictsStore.records
      .filter((r) => (showResolved ? true : r.status === 'open'))
      .sort((a, b) => (a.detectedAt < b.detectedAt ? 1 : -1))
  );

  const groupedByKind = $derived.by(() => {
    const groups = new Map<ConflictKind, ConflictRecord[]>();
    for (const record of visibleRecords) {
      const existing = groups.get(record.kind) ?? [];
      existing.push(record);
      groups.set(record.kind, existing);
    }
    return groups;
  });

  // Resolve participant ids to display labels lazily, as records render —
  // never re-deriving evidence, only a human-readable name for navigation.
  async function labelFor(nodeId: string): Promise<string> {
    if (participantLabels.has(nodeId)) return participantLabels.get(nodeId)!;
    try {
      const node = await backendAdapter.getNode(nodeId);
      const label = node?.content?.trim() || node?.title?.trim() || nodeId;
      participantLabels = new Map(participantLabels).set(nodeId, label);
      if (node?.nodeType) {
        participantTypes = new Map(participantTypes).set(nodeId, node.nodeType);
      }
      return label;
    } catch (e) {
      log.warn('Failed to resolve participant label', { error: e, nodeId });
      return nodeId;
    }
  }

  function openParticipant(nodeId: string) {
    const nodeType = participantTypes.get(nodeId) ?? 'text';
    getNavigationService().focusOrOpenNode(nodeId, { nodeType });
  }

  async function handleDismiss(conflictId: string) {
    try {
      await conflictsStore.dismiss(conflictId);
    } catch (e) {
      log.error('Failed to dismiss conflict', e);
    }
  }

  let mergingConflictId = $state<string | null>(null);
  /** Why the last merge attempt on a record did not happen, keyed by
   * conflict id; shown inline in that record's row. */
  let mergeErrors = $state<Map<string, string>>(new Map());

  function setMergeError(conflictId: string, message: string | null) {
    const next = new Map(mergeErrors);
    if (message === null) next.delete(conflictId);
    else next.set(conflictId, message);
    mergeErrors = next;
  }

  /** Resolve every id to a label first, so message builders stay sync. */
  async function resolveLabels(ids: (string | null)[]): Promise<LabelOf> {
    await Promise.all(ids.filter((id): id is string => id !== null).map(labelFor));
    return (id) => participantLabels.get(id) ?? id;
  }

  /** Turn a failed preview/merge into the inline message for its row. */
  async function reportMergeFailure(
    conflictId: string,
    survivorId: string,
    loserId: string,
    error: unknown
  ) {
    if (isTreeInvariantViolation(error)) {
      const violation = error.conflictData;
      const labelOf = await resolveLabels([
        violation.node_id,
        survivorId,
        loserId,
        ...violation.related_ids
      ]);
      setMergeError(conflictId, describeMergeRefusal(violation, survivorId, loserId, labelOf));
      return;
    }
    log.error('Failed to merge nodes', error);
    const detail =
      typeof error === 'object' && error !== null && 'message' in error ? error.message : null;
    setMergeError(
      conflictId,
      typeof detail === 'string' && detail ? `Merge failed: ${detail}` : 'Merge failed.'
    );
  }

  /** Merge is offered only for a 2-participant record — the shape ADR-068
   * §5.2 defines (a survivor and a loser). `survivorId` is whichever
   * participant the user clicked "Keep this one" for. The merge is previewed
   * first, so a refusal is explained before the user confirms and the
   * confirmation can say where the merged node will live. */
  async function handleMerge(record: ConflictRecord, survivorId: string) {
    const loserId = record.nodeIds.find((id) => id !== survivorId);
    if (!loserId) return;

    mergingConflictId = record.id;
    setMergeError(record.id, null);
    try {
      let preview: MergePreview;
      try {
        preview = await conflictsStore.previewMerge(survivorId, loserId);
      } catch (e) {
        await reportMergeFailure(record.id, survivorId, loserId, e);
        return;
      }

      const labelOf = await resolveLabels([
        survivorId,
        loserId,
        preview.survivorParentId,
        preview.loserParentId,
        preview.resultingParentId
      ]);
      const survivorLabel = labelOf(survivorId);
      const loserLabel = labelOf(loserId);
      const position = describeSurvivorPosition(preview, survivorId, loserId, labelOf);
      const confirmed = window.confirm(
        `Merge "${loserLabel}" into "${survivorLabel}"? ${loserLabel} will be archived; its ` +
          'properties and relationships move onto the surviving node.' +
          (position ? ` ${position}` : '') +
          ' This cannot be undone from here.'
      );
      if (!confirmed) return;

      try {
        await conflictsStore.merge(survivorId, loserId, record.id);
      } catch (e) {
        // The tree can change between the preview and the merge.
        await reportMergeFailure(record.id, survivorId, loserId, e);
      }
    } finally {
      mergingConflictId = null;
    }
  }

  /** Non-destructive: record that `adopted` was kept instead of treating the
   * other participant as a distinct node. Offered only for a 2-participant
   * `UniqueFieldCollision`/`CollectionNameCollision` record, same shape as
   * merge, but without touching either node's data. */
  async function handleAdoptExisting(record: ConflictRecord, adopted: string) {
    try {
      await conflictsStore.adoptExisting(record.id, adopted);
    } catch (e) {
      log.error('Failed to record adopt-existing resolution', e);
    }
  }

  let renamingConflictId = $state<string | null>(null);
  let renameDraft = $state('');

  function startRename(record: ConflictRecord, nodeId: string) {
    renamingConflictId = record.id;
    renameDraft = participantLabels.get(nodeId) ?? '';
  }

  function cancelRename() {
    renamingConflictId = null;
    renameDraft = '';
  }

  /** Collection-name collisions only: rename one participant so its name no
   * longer collides. */
  async function confirmRename(record: ConflictRecord, nodeId: string) {
    const from = participantLabels.get(nodeId) ?? nodeId;
    const to = renameDraft.trim();
    if (!to || to === from) {
      cancelRename();
      return;
    }
    try {
      await conflictsStore.rename(record.id, nodeId, from, to);
      participantLabels = new Map(participantLabels).set(nodeId, to);
    } catch (e) {
      log.error('Failed to rename collection', e);
    } finally {
      cancelRename();
    }
  }
</script>

<div class="conflicts-pane">
  <div class="conflicts-header">
    <h1>Conflicts</h1>
    <label class="show-resolved-toggle">
      <input type="checkbox" bind:checked={showResolved} />
      Show resolved &amp; dismissed
    </label>
  </div>

  <div class="conflicts-body">
    {#if !conflictsStore.loaded}
      <div class="conflicts-status">Loading…</div>
    {:else if visibleRecords.length === 0}
      <div class="conflicts-empty">
        <p>No open conflicts.</p>
        <p class="conflicts-empty-sub">Everything here converges cleanly.</p>
      </div>
    {:else}
      {#each groupedByKind.entries() as [kind, records] (kind)}
        <section class="conflict-group">
          <h2>{KIND_LABELS[kind] ?? kind}</h2>
          <ul class="conflict-list">
            {#each records as record (record.id)}
              <li class="conflict-row" class:resolved={record.status !== 'open'}>
                <div class="conflict-participants">
                  {#each record.nodeIds as nodeId, i (nodeId)}
                    {#if i > 0}<span class="conflict-sep">↔</span>{/if}
                    <!-- eslint-disable-next-line -->
                    {#await labelFor(nodeId)}
                      <span class="participant-link">{nodeId}</span>
                    {:then label}
                      <button
                        type="button"
                        class="participant-link"
                        onclick={() => openParticipant(nodeId)}
                      >
                        {label}
                      </button>
                    {/await}
                  {/each}
                </div>

                <div class="conflict-meta">
                  <span class="conflict-status">{record.status}</span>
                  {#if record.occurrences > 1}
                    <span class="conflict-occurrences">×{record.occurrences}</span>
                  {/if}
                  <span class="conflict-detected-at">{record.detectedAt}</span>
                </div>

                {#if record.status === 'open'}
                  {#if renamingConflictId === record.id}
                    <div class="conflict-rename-form">
                      <input
                        type="text"
                        class="conflict-rename-input"
                        bind:value={renameDraft}
                        placeholder="New name"
                      />
                      <button
                        type="button"
                        class="conflict-action"
                        onclick={() => confirmRename(record, record.nodeIds[1])}
                      >
                        Save
                      </button>
                      <button type="button" class="conflict-action" onclick={cancelRename}>
                        Cancel
                      </button>
                    </div>
                  {:else}
                    <div class="conflict-actions">
                      {#if record.nodeIds.length === 2}
                        <!-- Merge (ADR-068 §5.2): user-initiated only, one
                             button per participant to pick which one survives. -->
                        {#each record.nodeIds as nodeId (nodeId)}
                          <button
                            type="button"
                            class="conflict-action"
                            disabled={mergingConflictId === record.id}
                            onclick={() => handleMerge(record, nodeId)}
                          >
                            Keep {participantLabels.get(nodeId) ?? nodeId}
                          </button>
                        {/each}
                        <!-- Adopt existing (§5.1): non-destructive — creates
                             nothing, deletes nothing, just records which
                             participant is the "real" one going forward. -->
                        <button
                          type="button"
                          class="conflict-action"
                          onclick={() => handleAdoptExisting(record, record.nodeIds[0])}
                        >
                          Adopt existing
                        </button>
                      {/if}
                      {#if record.kind === 'collection_name_collision'}
                        <!-- Rename (§5.1, collection-name collisions only):
                             an ordinary update_node on one participant. -->
                        <button
                          type="button"
                          class="conflict-action"
                          onclick={() => startRename(record, record.nodeIds[1])}
                        >
                          Rename
                        </button>
                      {/if}
                      <button
                        type="button"
                        class="conflict-action"
                        onclick={() => handleDismiss(record.id)}
                      >
                        Dismiss
                      </button>
                    </div>
                  {/if}
                {/if}
                {#if record.status === 'open' && mergeErrors.has(record.id)}
                  <p class="conflict-error" role="alert">{mergeErrors.get(record.id)}</p>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {/each}
    {/if}
  </div>
</div>

<style>
  .conflicts-pane {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: hsl(var(--background));
  }

  .conflicts-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1.5rem 2rem 0.75rem;
  }

  .conflicts-header h1 {
    margin: 0;
    font-size: 1.25rem;
    font-weight: 600;
  }

  .show-resolved-toggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
    color: hsl(var(--muted-foreground));
    cursor: pointer;
  }

  .conflicts-body {
    flex: 1;
    overflow-y: auto;
    padding: 0.5rem 2rem 1.5rem;
  }

  .conflicts-status,
  .conflicts-empty {
    padding: 2rem 1rem;
    color: hsl(var(--muted-foreground));
    font-size: 0.9rem;
  }

  .conflicts-empty-sub {
    margin-top: 0.25rem;
    font-size: 0.8rem;
  }

  .conflict-group {
    margin-bottom: 1.5rem;
  }

  .conflict-group h2 {
    font-size: 0.95rem;
    font-weight: 600;
    margin: 0 0 0.5rem;
  }

  .conflict-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .conflict-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.6rem 0.75rem;
    border-radius: 0.375rem;
    border: 1px solid hsl(var(--border));
    margin-bottom: 0.4rem;
  }

  .conflict-row.resolved {
    opacity: 0.6;
  }

  .conflict-participants {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
    min-width: 0;
  }

  .conflict-sep {
    color: hsl(var(--muted-foreground));
  }

  .participant-link {
    background: none;
    border: none;
    padding: 0;
    color: hsl(var(--primary));
    cursor: pointer;
    text-decoration: underline;
    font-size: 0.9rem;
  }

  .conflict-meta {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    flex-shrink: 0;
  }

  .conflict-actions {
    flex-shrink: 0;
    display: flex;
    gap: 0.4rem;
  }

  .conflict-action {
    font-size: 0.8rem;
    padding: 0.25rem 0.6rem;
    border-radius: 0.3rem;
    border: 1px solid hsl(var(--border));
    background: hsl(var(--muted));
    cursor: pointer;
  }

  .conflict-action:hover {
    background: hsl(var(--accent));
  }

  .conflict-error {
    flex-basis: 100%;
    margin: 0;
    font-size: 0.8rem;
    color: hsl(var(--destructive));
  }

  .conflict-rename-form {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-shrink: 0;
  }

  .conflict-rename-input {
    font-size: 0.85rem;
    padding: 0.25rem 0.5rem;
    border-radius: 0.3rem;
    border: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    color: hsl(var(--foreground));
  }
</style>
