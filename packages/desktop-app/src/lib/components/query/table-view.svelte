<!--
  TableView - Pure table rendering component for QueryNodeViewer

  Derives columns from schema field definitions (not by enumerating result node keys).
  Always includes 'content' (title) as the first column — rendered as a clickable link.
  Additional columns: one per user-visible schema field definition, in schema order,
  using field.friendlyName. `protection: 'system'` fields are excluded — see
  isUserVisibleField.
  Clicking the content/title cell calls onRowClick(node.id).
  Results are paginated at 25 rows per page.
-->

<script lang="ts">
  import type { SchemaField, SchemaNode } from '$lib/types/schema-node';
  import { labelForField } from '$lib/utils/schema-field-label';
  import { isUserVisibleField } from '$lib/utils/schema-field-visibility';
  import TableRow from '$lib/components/query/table-row.svelte';
  import { Table, TableHeader, TableBody, TableHead, TableRow as UiTableRow } from '$lib/components/ui/table';
  import { Button } from '$lib/components/ui/button';

  let {
    nodeIds,
    schema,
    fieldSchemaMap,
    onRowClick
  }: {
    nodeIds: string[];
    schema: SchemaNode | null;
    fieldSchemaMap: Map<string, SchemaField>;
    onRowClick: (_nodeId: string) => void;
  } = $props();

  const PAGE_SIZE = 25;
  // Raw user pagination intent. The effective page is derived and clamped into the
  // valid range for the current nodeIds, so a shrinking/changing result set corrects an
  // out-of-range page on read — no $effect syncing state to the nodeIds prop (ADR-049).
  let currentPage = $state(0);

  // Derive columns from schema fields, skipping system-managed ones — a column
  // for a field the user can never fill (and which may hold internals like
  // a PTY chat's raw `transcript`) is not a user-facing column. Same
  // predicate the detail form uses, so the two views agree.
  const columns = $derived.by(() => {
    const cols: Array<{ field: string; label: string }> = [
      { field: 'content', label: '' }
    ];

    if (schema?.fields) {
      for (const field of schema.fields.filter(isUserVisibleField)) {
        cols.push({ field: field.name, label: labelForField(field) });
      }
    }

    return cols;
  });

  const totalPages = $derived(Math.ceil(nodeIds.length / PAGE_SIZE));

  // Effective page, clamped so it never points past the current result set.
  const page = $derived(Math.min(currentPage, Math.max(0, totalPages - 1)));

  const pageIds = $derived(nodeIds.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE));

</script>

<Table>
  <TableHeader>
    <UiTableRow>
      {#each columns as col (col.field)}
        <TableHead>{col.label}</TableHead>
      {/each}
    </UiTableRow>
  </TableHeader>
  <TableBody>
    {#each pageIds as id (id)}
      <TableRow {id} {columns} {fieldSchemaMap} {onRowClick} />
    {/each}
  </TableBody>
</Table>

{#if totalPages > 1}
  <div class="border-border flex items-center justify-center gap-3 border-t p-4">
    <Button
      variant="outline"
      size="sm"
      onclick={() => (currentPage = page - 1)}
      disabled={page === 0}
    >
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
