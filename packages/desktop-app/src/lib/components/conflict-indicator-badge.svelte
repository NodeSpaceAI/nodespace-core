<!--
  Inline per-node conflict indicator (ADR-068, conflict-journal-and-resolution.md
  §6.3): a derived read of the conflict journal — never a stored property —
  that links into the Conflicts view rather than trying to resolve in a
  popover. Renders nothing unless this node is named by an open conflict
  record.

  This replaces both `possible-duplicate-badge.svelte` (deleted in S1 — a
  boolean marker that could never be cleared) and `recovered-items-badge.svelte`
  (deleted here — its `SupersededEdit` data now lives in this same journal):
  one indicator, one surface, one dismiss mechanism.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { conflictsStore } from '$lib/stores/conflicts.svelte';
  import { openConflicts } from '$lib/utils/open-conflicts';
  import { Badge } from '$lib/components/ui/badge';
  import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';

  let { nodeId }: { nodeId: string } = $props();

  const hasOpen = $derived(conflictsStore.hasOpenFor(nodeId));

  // Per-node lookup on mount so the indicator can light up without requiring
  // the full Conflicts view to have loaded first in this session.
  onMount(() => {
    void conflictsStore.loadForNode(nodeId);
  });
</script>

{#if hasOpen}
  <button type="button" class="conflict-indicator-trigger" onclick={openConflicts}>
    <Badge variant="outline" class="border-amber-500 text-amber-700 dark:text-amber-400">
      <TriangleAlertIcon class="h-3 w-3" />
      Conflict
    </Badge>
  </button>
{/if}

<style>
  .conflict-indicator-trigger {
    display: inline-flex;
    align-self: flex-start;
    padding: 0;
    background: none;
    border: none;
    cursor: pointer;
  }
</style>
