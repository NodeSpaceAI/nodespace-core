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
  import QueryEditor from '$lib/components/query/query-editor.svelte';
  import type { QueryDefinition } from '$lib/components/query/query-editor.svelte';
  import type { SchemaNode, SchemaField } from '$lib/types/schema-node';
  import type { Node } from '$lib/types';
  import { createLogger } from '$lib/utils/logger';
  import { queryPreferencesService } from '$lib/services/query-preferences-service';
  import type { QueryPreferences } from '$lib/types/query-preferences';

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

  // Edit mode state
  let isEditMode = $state(false);
  /** Raw node used for version tracking during property updates */
  let rawNode = $state<Node | null>(null);

  // View state — persisted per query node via QueryPreferencesService
  let activeView = $state<QueryPreferences['lastView']>('table');
  // Shown when a non-implemented view tab is clicked
  let viewComingSoon = $state(false);

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
    viewComingSoon = false;

    // Restore persisted view preference for this query node (synchronous)
    const prefs = queryPreferencesService.getPreferences(schemaId);
    activeView = prefs.lastView;

    try {
      // Load raw node for property editing (version tracking)
      const raw = await backendAdapter.getNode(schemaId);
      if (loadId !== currentLoadId) return;
      rawNode = raw;

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
      const message = e instanceof Error ? e.message : String(e);

      // Schema not found on a fresh database is expected — show empty state, not error
      if (isSchemaNotFound(message)) {
        log.debug('Schema not yet created, showing empty state', { schemaId });
        queryState = 'success';
        return;
      }

      log.error('Failed to load schema node', { schemaId, error: message });
      error = message;
      queryState = 'error';
    }
  }

  function isSchemaNotFound(message: string): boolean {
    // Tauri CommandError: "Schema '<id>' not found" (code: SCHEMA_NOT_FOUND)
    // HTTP adapter: parsed from ApiError with code SCHEMA_NOT_FOUND
    return /Schema '.*' not found/.test(message) || message.includes('SCHEMA_NOT_FOUND');
  }

  // Build a lookup map from schema fields for enum label resolution
  const fieldSchemaMap = $derived.by(() => {
    const map = new Map<string, SchemaField>();
    if (schemaNode?.fields) {
      for (const f of schemaNode.fields) map.set(f.name, f);
    }
    return map;
  });

  /** Extract the current QueryDefinition from the raw node's properties */
  const currentQueryDefinition = $derived.by((): QueryDefinition | null => {
    const props = rawNode?.properties;
    if (!props || typeof props.targetType !== 'string') return null;
    return {
      targetType: props.targetType,
      filters: Array.isArray(props.filters) ? (props.filters as QueryDefinition['filters']) : [],
      sorting: Array.isArray(props.sorting) ? (props.sorting as QueryDefinition['sorting']) : undefined,
      limit: typeof props.limit === 'number' ? props.limit : undefined,
    };
  });

  async function handleQuerySave(definition: QueryDefinition): Promise<void> {
    if (!rawNode) {
      log.warn('QueryNodeViewer: cannot save — raw node not loaded');
      return;
    }
    try {
      const updated = await backendAdapter.updateNode(rawNode.id, rawNode.version, {
        properties: {
          ...rawNode.properties,
          targetType: definition.targetType,
          filters: definition.filters,
          ...(definition.sorting !== undefined ? { sorting: definition.sorting } : {}),
          ...(definition.limit !== undefined ? { limit: definition.limit } : {}),
        },
      });
      rawNode = updated;
      isEditMode = false;
      log.debug('QueryNodeViewer: query definition saved', { nodeId: rawNode.id });
      // Re-execute the query with the updated definition
      untrack(() => loadAndQuery(nodeId));
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      log.error('QueryNodeViewer: failed to save query definition', { error: message });
    }
  }

  function handleQueryCancel(): void {
    isEditMode = false;
  }

  function handleViewChange(view: QueryPreferences['lastView']): void {
    activeView = view;
    viewComingSoon = view !== 'table';
    queryPreferencesService.saveViewConfig(nodeId, view);
  }

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
    <nav class="view-tabs" aria-label="View options">
      <button
        class="view-tab"
        class:active={activeView === 'list'}
        onclick={() => handleViewChange('list')}
        aria-pressed={activeView === 'list'}
      >List</button>
      <button
        class="view-tab"
        class:active={activeView === 'table'}
        onclick={() => handleViewChange('table')}
        aria-pressed={activeView === 'table'}
      >Table</button>
      <button
        class="view-tab"
        class:active={activeView === 'kanban'}
        onclick={() => handleViewChange('kanban')}
        aria-pressed={activeView === 'kanban'}
      >Kanban</button>
    </nav>
    {#if rawNode && !isEditMode}
      <button class="edit-query-button" onclick={() => { isEditMode = true; }}>Edit Query</button>
    {/if}
  </header>

  {#if isEditMode}
    <div class="edit-mode-wrapper">
      <QueryEditor
        query={currentQueryDefinition}
        onSave={handleQuerySave}
        onCancel={handleQueryCancel}
      />
    </div>
  {/if}

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
    {:else if queryState === 'success' && viewComingSoon}
      <div class="coming-soon-state">
        <p>The <strong>{activeView}</strong> view is coming soon.</p>
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

  .edit-query-button {
    padding: 0.25rem 0.625rem;
    font-size: 0.8125rem;
    font-weight: 500;
    background: hsl(var(--secondary));
    color: hsl(var(--secondary-foreground));
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    cursor: pointer;
    transition: background-color 0.15s ease;
    flex-shrink: 0;
  }

  .edit-query-button:hover {
    background: hsl(var(--muted));
  }

  .edit-mode-wrapper {
    padding: 1rem 2rem;
    border-bottom: 1px solid hsl(var(--border));
  }

  .view-tabs {
    display: flex;
    gap: 0.125rem;
    background: hsl(var(--muted));
    border-radius: 0.375rem;
    padding: 0.125rem;
    flex-shrink: 0;
  }

  .view-tab {
    padding: 0.25rem 0.625rem;
    font-size: 0.8125rem;
    font-weight: 500;
    background: transparent;
    color: hsl(var(--muted-foreground));
    border: none;
    border-radius: 0.25rem;
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease;
    white-space: nowrap;
  }

  .view-tab:hover {
    color: hsl(var(--foreground));
    background: hsl(var(--muted) / 0.6);
  }

  .view-tab.active {
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    box-shadow: 0 1px 2px hsl(var(--border) / 0.5);
  }

  .coming-soon-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 3rem;
    text-align: center;
    color: hsl(var(--muted-foreground));
    gap: 1rem;
  }

  .coming-soon-state p {
    margin: 0;
    font-size: 1rem;
  }

  .coming-soon-state strong {
    color: hsl(var(--foreground));
    text-transform: capitalize;
  }
</style>
