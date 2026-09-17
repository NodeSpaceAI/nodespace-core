<!--
  ListView - Simple title list for QueryNodeViewer

  One row per result, showing the node's current display value via
  pluginRegistry.resolveDisplayTitle (title only for title_template-driven schemas, content
  otherwise — see node-display-title.ts), falling back to "Untitled". Reads each node
  from sharedNodeStore so rows stay reactive to edits made in another pane. Clicking a
  row opens the node via onRowClick. Paginated at the same PAGE_SIZE as TableView for
  consistency.
-->

<script lang="ts">
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { pluginRegistry } from '$lib/plugins/plugin-registry';
  import { Button } from '$lib/components/ui/button';

  let {
    nodeIds,
    onRowClick
  }: {
    nodeIds: string[];
    onRowClick: (_nodeId: string) => void;
  } = $props();

  const PAGE_SIZE = 25;
  // Raw user pagination intent; the effective page is derived and clamped so a
  // shrinking result set corrects an out-of-range page on read (ADR-049).
  let currentPage = $state(0);
  const totalPages = $derived(Math.ceil(nodeIds.length / PAGE_SIZE));
  const page = $derived(Math.min(currentPage, Math.max(0, totalPages - 1)));
  const pageIds = $derived(nodeIds.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE));
</script>

<ul class="list-view">
  {#each pageIds as id (id)}
    {@const node = sharedNodeStore.getNode(id)}
    {#if node}
      {@const title = pluginRegistry.resolveDisplayTitle(node) || 'Untitled'}
      <li>
        <button class="list-row" onclick={() => onRowClick(id)} title={`Open ${title}`}>
          {title}
        </button>
      </li>
    {/if}
  {/each}
</ul>

{#if totalPages > 1}
  <div class="border-border flex items-center justify-center gap-3 border-t p-4">
    <Button variant="outline" size="sm" onclick={() => (currentPage = page - 1)} disabled={page === 0}>
      ‹
    </Button>
    <span class="text-muted-foreground text-sm">{page + 1} / {totalPages}</span>
    <Button
      variant="outline"
      size="sm"
      onclick={() => (currentPage = page + 1)}
      disabled={page >= totalPages - 1}
    >
      ›
    </Button>
  </div>
{/if}

<style>
  .list-view {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }

  .list-row {
    display: block;
    width: 100%;
    text-align: left;
    padding: 0.5rem 0.25rem;
    font-size: 0.875rem;
    color: hsl(var(--foreground));
    background: transparent;
    border: none;
    border-bottom: 1px solid hsl(var(--border));
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .list-row:hover {
    color: hsl(var(--primary));
    background: hsl(var(--muted) / 0.5);
  }
</style>
