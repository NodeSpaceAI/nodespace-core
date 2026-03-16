<!--
  TableView - Pure table rendering component for QueryNodeViewer

  Derives columns from schema field definitions (not by enumerating result node keys).
  Always includes 'content' (title) as the first column — rendered as a clickable link.
  Additional columns: one per schema field definition, in schema order.
  Clicking the content/title cell calls onRowClick(node.id).
-->

<script lang="ts">
  import type { Node } from '$lib/types';
  import type { SchemaNode } from '$lib/types/schema-node';

  let {
    results,
    schema,
    onRowClick
  }: {
    results: Node[];
    schema: SchemaNode | null;
    onRowClick: (_nodeId: string) => void;
  } = $props();

  // Derive columns from schema fields — schema-driven, not key enumeration
  const columns = $derived.by(() => {
    const cols: Array<{ field: string; label: string }> = [
      { field: 'content', label: 'Title' }
    ];

    if (schema?.fields) {
      for (const field of schema.fields) {
        cols.push({ field: field.name, label: field.name.charAt(0).toUpperCase() + field.name.slice(1) });
      }
    }

    return cols;
  });

  function getCellValue(node: Node, field: string): string {
    if (field === 'content') {
      return node.content || '';
    }
    const val = node.properties?.[field];
    if (val === null || val === undefined) return '';
    if (typeof val === 'object') return JSON.stringify(val);
    return String(val);
  }
</script>

<div class="table-view">
  <table>
    <thead>
      <tr>
        {#each columns as col (col.field)}
          <th>{col.label}</th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each results as node (node.id)}
        <tr class="result-row">
          {#each columns as col, i (col.field)}
            <td>
              {#if i === 0}
                <button
                  class="title-link"
                  onclick={() => onRowClick(node.id)}
                  title="Open {node.content || 'node'} in other panel"
                >
                  {getCellValue(node, col.field) || 'Untitled'}
                </button>
              {:else}
                <span class="cell-value">{getCellValue(node, col.field)}</span>
              {/if}
            </td>
          {/each}
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .table-view {
    width: 100%;
    overflow-x: auto;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.875rem;
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
    max-width: 300px;
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
  }

  .title-link:hover {
    color: hsl(var(--primary));
    text-decoration: underline;
  }

  .cell-value {
    color: hsl(var(--muted-foreground));
  }
</style>
