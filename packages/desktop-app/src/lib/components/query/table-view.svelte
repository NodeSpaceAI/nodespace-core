<!--
  TableView - Pure table rendering component for QueryNodeViewer

  Derives columns from schema field definitions (not by enumerating result node keys).
  Always includes 'content' (title) as the first column — rendered as a clickable link.
  Additional columns: one per schema field definition, in schema order, using field.label.
  Clicking the content/title cell calls onRowClick(node.id).
  Results are paginated at 25 rows per page.
-->

<script lang="ts">
  import type { Node } from '$lib/types';
  import type { SchemaNode } from '$lib/types/schema-node';

  let {
    results,
    schema,
    getFieldValue,
    onRowClick
  }: {
    results: Node[];
    schema: SchemaNode | null;
    getFieldValue: (_node: Node, _field: string) => string;
    onRowClick: (_nodeId: string) => void;
  } = $props();

  const PAGE_SIZE = 25;
  let currentPage = $state(0);

  // Reset to page 0 when results change
  $effect(() => {
    results;
    currentPage = 0;
  });

  // Derive columns from schema fields — capitalize name and replace underscores with spaces
  const columns = $derived.by(() => {
    const cols: Array<{ field: string; label: string }> = [
      { field: 'content', label: 'Title' }
    ];

    if (schema?.fields) {
      for (const field of schema.fields) {
        const label = field.name
          .replace(/_/g, ' ')
          .replace(/([a-z])([A-Z])/g, '$1 $2')
          .replace(/^\w/, (c) => c.toUpperCase());
        cols.push({ field: field.name, label });
      }
    }

    return cols;
  });

  const totalPages = $derived(Math.ceil(results.length / PAGE_SIZE));

  const pageResults = $derived(
    results.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE)
  );

</script>

<div class="table-wrapper">
  <table>
    <thead>
      <tr>
        {#each columns as col (col.field)}
          <th>{col.label}</th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each pageResults as node (node.id)}
        <tr class="result-row">
          {#each columns as col (col.field)}
            <td>
              {#if col.field === 'content'}
                <button
                  class="title-link"
                  onclick={() => onRowClick(node.id)}
                  title="Open {node.content || 'node'}"
                >
                  {getFieldValue(node, col.field) || 'Untitled'}
                </button>
              {:else}
                <span class="cell-value">{getFieldValue(node, col.field)}</span>
              {/if}
            </td>
          {/each}
        </tr>
      {/each}
    </tbody>
  </table>

  {#if totalPages > 1}
    <div class="pagination">
      <button
        class="page-btn"
        onclick={() => currentPage--}
        disabled={currentPage === 0}
      >
        ‹
      </button>
      <span class="page-info">{currentPage + 1} / {totalPages}</span>
      <button
        class="page-btn"
        onclick={() => currentPage++}
        disabled={currentPage >= totalPages - 1}
      >
        ›
      </button>
    </div>
  {/if}
</div>

<style>
  .table-wrapper {
    width: 100%;
    overflow: hidden;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.875rem;
    table-layout: fixed;
  }

  thead {
    position: sticky;
    top: 0;
    background: hsl(var(--background));
    z-index: 1;
  }

  th {
    padding: 0.75rem 1rem;
    text-align: left;
    font-weight: 600;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: hsl(var(--muted-foreground));
    border-bottom: 2px solid hsl(var(--border));
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .result-row {
    transition: background-color 0.1s ease;
  }

  .result-row:hover {
    background: hsl(var(--muted));
  }

  td {
    padding: 0.75rem 1rem;
    border-bottom: 1px solid hsl(var(--border));
    vertical-align: middle;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title-link {
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    font-size: 0.875rem;
    color: hsl(var(--foreground));
    font-weight: 500;
    text-align: left;
    transition: color 0.15s ease;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }

  .title-link:hover {
    color: hsl(var(--primary));
    text-decoration: underline;
  }

  .cell-value {
    color: hsl(var(--muted-foreground));
  }

  .pagination {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.75rem;
    padding: 1rem;
    border-top: 1px solid hsl(var(--border));
  }

  .page-btn {
    background: hsl(var(--secondary));
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    padding: 0.25rem 0.625rem;
    cursor: pointer;
    font-size: 1rem;
    color: hsl(var(--foreground));
    line-height: 1;
  }

  .page-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .page-info {
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
  }
</style>
