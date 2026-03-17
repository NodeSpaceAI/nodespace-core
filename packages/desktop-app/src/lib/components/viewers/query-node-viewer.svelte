<!--
  QueryNodeViewer - Page-level viewer for displaying query results

  Loads a schema node by nodeId, derives column definitions from its fields,
  executes a query for all nodes matching the schema's targetType, and renders
  results in a TableView. Row clicks open the node in another panel.

  This is the target for Schema Types sidenav navigation — clicking a schema
  type passes the schema's nodeId directly (every schema is itself a node).

  Follows the *NodeViewer pattern but does NOT wrap BaseNodeViewer because
  it shows flat query results rather than a hierarchical node collection.
-->

<script lang="ts">
  import { untrack } from 'svelte';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { tabState, setActiveTab } from '$lib/stores/navigation.js';
  import { get } from 'svelte/store';
  import TableView from '$lib/components/query/table-view.svelte';
  import type { SchemaNode, SchemaField } from '$lib/types/schema-node';
  import { createLogger } from '$lib/utils/logger';

  const log = createLogger('QueryNodeViewer');

  let {
    nodeId,
    paneId,
    onTitleChange
  }: {
    nodeId: string;
    paneId?: string;
    onTitleChange?: (_title: string) => void;
  } = $props();

  let schemaNode = $state<SchemaNode | null>(null);
  // IDs of nodes loaded for this schema type.
  // TableView calls sharedNodeStore.getNode(id) per row inside its reactive template,
  // which is how task-node.svelte achieves live reactivity — the lookup happens inside
  // the Svelte component's tracked context, not in a pre-computed $derived array.
  let loadedNodeIds = $state<string[]>([]);
  let queryState = $state<'idle' | 'loading' | 'success' | 'error'>('idle');
  let error = $state<string | null>(null);
  // Sentinel to discard in-flight responses when nodeId changes rapidly (sidenav navigation)
  let currentLoadId = $state(0);

  const hasResults = $derived(loadedNodeIds.length > 0);

  // Load schema and execute query when nodeId changes
  $effect(() => {
    const id = nodeId;
    untrack(() => loadAndQuery(id));
  });

  // Update tab title when schema node is loaded
  $effect(() => {
    if (schemaNode) {
      untrack(() => onTitleChange?.(schemaNode!.content || 'Query'));
    }
  });

  async function loadAndQuery(schemaId: string) {
    const loadId = ++currentLoadId;
    queryState = 'loading';
    error = null;
    schemaNode = null;
    loadedNodeIds = [];

    try {
      const schema = await backendAdapter.getSchema(schemaId);
      if (loadId !== currentLoadId) return;
      schemaNode = schema;
      log.debug('Loaded schema node', { schemaId, content: schema.content });

      // Fetch all nodes of this type and load them into the shared store.
      // TableRow components use $derived(sharedNodeStore.getNode(id)) per row,
      // so updates from other panes propagate reactively without re-querying.
      const nodes = await backendAdapter.queryNodes({ nodeType: schema.id });
      if (loadId !== currentLoadId) return;
      const databaseSource = { type: 'database' as const, reason: 'query-node-viewer initial load' };
      for (const node of nodes) {
        sharedNodeStore.setNode(node, databaseSource);
      }
      loadedNodeIds = nodes.map((n) => n.id);
      queryState = 'success';
      log.debug('Query loaded into store', { schemaId: schema.id, count: nodes.length });
    } catch (e) {
      if (loadId !== currentLoadId) return;
      const message = e instanceof Error ? e.message : 'Failed to load schema';
      log.error('Failed to load schema node', { schemaId, error: message });
      error = message;
      queryState = 'error';
    }
  }

  // Build a lookup map from schema fields for enum label resolution
  const fieldSchemaMap = $derived.by(() => {
    const map = new Map<string, SchemaField>();
    if (schemaNode?.fields) {
      for (const f of schemaNode.fields) map.set(f.name, f);
    }
    return map;
  });

  function handleRowClick(clickedNodeId: string) {
    // Check if node is already open in any tab — if so, switch to it
    const state = get(tabState);
    const existingTab = state.tabs.find((t) => t.content?.nodeId === clickedNodeId);
    if (existingTab) {
      setActiveTab(existingTab.id, existingTab.paneId);
      return;
    }
    getNavigationService().navigateToNodeInOtherPane(clickedNodeId, paneId);
  }
</script>

<div class="query-node-viewer">
  <header class="query-header">
    <h1>{schemaNode?.content ?? 'Query'}</h1>
    {#if queryState === 'success'}
      <span class="result-count">{loadedNodeIds.length} {loadedNodeIds.length === 1 ? 'item' : 'items'}</span>
    {/if}
  </header>

  <div class="query-content">
    {#if queryState === 'loading'}
      <div class="loading-state">
        <span>Loading...</span>
      </div>
    {:else if queryState === 'error'}
      <div class="error-state">
        <span>{error}</span>
        <button class="retry-button" onclick={() => loadAndQuery(nodeId)}>Retry</button>
      </div>
    {:else if queryState === 'success' && !hasResults}
      <div class="empty-state">
        <p>No nodes of this type yet.</p>
      </div>
    {:else if queryState === 'success'}
      <TableView nodeIds={loadedNodeIds} schema={schemaNode} {fieldSchemaMap} onRowClick={handleRowClick} />
    {/if}
  </div>
</div>

<style>
  .query-node-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: hsl(var(--background));
  }

  .query-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 1.5rem 2rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    flex-shrink: 0;
  }

  .query-header h1 {
    font-size: 1.5rem;
    font-weight: 600;
    margin: 0;
    color: hsl(var(--foreground));
    flex: 1;
  }

  .result-count {
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
    padding: 0.25rem 0.5rem;
    background: hsl(var(--muted));
    border-radius: 9999px;
  }

  .query-content {
    flex: 1;
    overflow-y: auto;
    padding: 1rem 2rem;
  }

  .loading-state,
  .error-state,
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 3rem;
    text-align: center;
    color: hsl(var(--muted-foreground));
    gap: 1rem;
  }

  .error-state {
    color: hsl(var(--destructive));
  }

  .retry-button {
    padding: 0.5rem 1rem;
    font-size: 0.875rem;
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
    border: none;
    border-radius: 0.375rem;
    cursor: pointer;
    transition: opacity 0.15s ease;
  }

  .retry-button:hover {
    opacity: 0.9;
  }

  .empty-state p {
    margin: 0;
    font-size: 1rem;
  }
</style>
