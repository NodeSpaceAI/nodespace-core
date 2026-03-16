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
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { getNavigationService } from '$lib/services/navigation-service';
  import TableView from '$lib/components/query/table-view.svelte';
  import type { Node } from '$lib/types';
  import type { SchemaNode } from '$lib/types/schema-node';
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
  let results = $state<Node[]>([]);
  let queryState = $state<'idle' | 'loading' | 'success' | 'error'>('idle');
  let error = $state<string | null>(null);

  // Load schema and execute query when nodeId changes
  $effect(() => {
    loadAndQuery(nodeId);
  });

  // Update tab title when schema node is loaded
  $effect(() => {
    if (schemaNode) {
      onTitleChange?.(schemaNode.content || 'Query');
    }
  });

  async function loadAndQuery(schemaId: string) {
    queryState = 'loading';
    error = null;
    schemaNode = null;
    results = [];

    try {
      // Load the schema node — nodeId IS the schema id (e.g. "task")
      const schema = await backendAdapter.getSchema(schemaId);
      schemaNode = schema;
      log.debug('Loaded schema node', { schemaId, content: schema.content });

      await executeQuery(schema);
    } catch (e) {
      const message = e instanceof Error ? e.message : 'Failed to load schema';
      log.error('Failed to load schema node', { schemaId, error: message });
      error = message;
      queryState = 'error';
    }
  }

  async function executeQuery(schema?: SchemaNode) {
    const target = schema ?? schemaNode;
    if (!target) return;

    queryState = 'loading';
    error = null;

    try {
      const nodes = await backendAdapter.queryNodes({ nodeType: target.id });
      results = nodes;
      queryState = 'success';
      log.debug('Query executed', { schemaId: target.id, resultCount: nodes.length });
    } catch (e) {
      const message = e instanceof Error ? e.message : 'Failed to execute query';
      log.error('Query execution failed', { schemaId: target.id, error: message });
      error = message;
      queryState = 'error';
    }
  }

  function handleRowClick(clickedNodeId: string) {
    getNavigationService().navigateToNodeInOtherPane(clickedNodeId, paneId);
  }

  function handleRefresh() {
    if (schemaNode) {
      executeQuery();
    } else {
      loadAndQuery(nodeId);
    }
  }
</script>

<div class="query-node-viewer">
  <header class="query-header">
    <h1>{schemaNode?.content ?? 'Loading...'}</h1>
    {#if queryState === 'success'}
      <span class="result-count">{results.length} {results.length === 1 ? 'item' : 'items'}</span>
    {/if}
    <button class="refresh-button" onclick={handleRefresh} disabled={queryState === 'loading'}>
      Refresh
    </button>
  </header>

  <div class="query-content">
    {#if queryState === 'loading'}
      <div class="loading-state">
        <span>Loading...</span>
      </div>
    {:else if queryState === 'error'}
      <div class="error-state">
        <span>{error}</span>
        <button class="retry-button" onclick={handleRefresh}>Retry</button>
      </div>
    {:else if queryState === 'success' && results.length === 0}
      <div class="empty-state">
        <p>No nodes of this type yet.</p>
      </div>
    {:else if queryState === 'success'}
      <TableView {results} schema={schemaNode} onRowClick={handleRowClick} />
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

  .refresh-button {
    padding: 0.375rem 0.75rem;
    font-size: 0.875rem;
    background: hsl(var(--secondary));
    color: hsl(var(--secondary-foreground));
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .refresh-button:hover:not(:disabled) {
    background: hsl(var(--muted));
  }

  .refresh-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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
