<!--
  Conflicts view (ADR-068, conflict-journal-and-resolution.md §6.2): a flat
  list of conflict records, grouped by kind, replacing the three half-places
  that existed before this ADR (the possible-duplicate badge, the
  conflict-notifications toast cap, and the Recovered Items badge/store).

  Open records are shown by default; resolved/dismissed are reachable behind
  a filter toggle, never deleted — the record of what a user decided is the
  point (§6.2). Empty state reads as healthy, not as an error.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    conflictsStore,
    type ConflictRecord,
    type ConflictKind
  } from '$lib/stores/conflicts.svelte';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('ConflictsPane');

  let showResolved = $state(false);
  let participantLabels = $state<Map<string, string>>(new Map());
  let participantTypes = $state<Map<string, string>>(new Map());

  const KIND_LABELS: Record<ConflictKind, string> = {
    unique_field_collision: 'Duplicate field value',
    collection_name_collision: 'Duplicate collection name',
    superseded_edit: 'Superseded edit',
    duplicate_reactive_create: 'Duplicate automated creation'
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
                  <div class="conflict-actions">
                    <button
                      type="button"
                      class="conflict-action"
                      onclick={() => handleDismiss(record.id)}
                    >
                      Dismiss
                    </button>
                  </div>
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
</style>
