<!--
  QueryNodeViewer - Page-level viewer for a type's default view and saved queries

  Serves two shapes from one component, branching on the node it was handed:

  - schema node → the DEFAULT type view: all nodes of the type, unfiltered,
    nothing persisted. The first divergence — a filter edit, a view/group-by
    change, or a title rename — MATERIALIZES a real `nodeType: 'query'` node and
    re-routes the tab to it, so subsequent edits persist.
  - query node → a SAVED query: reads the stored QueryDefinition + view config,
    executes with filters (client-side — queryNodes only filters by nodeType),
    and persists edits back onto the node.

  Row clicks open the node in another panel. Follows the *NodeViewer pattern but
  does NOT wrap BaseNodeViewer because it shows flat query results rather than a
  hierarchical node collection.
-->

<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { v4 as uuidv4 } from 'uuid';
  import { backendAdapter } from '$lib/services/backend-adapter';
  import { createSchemaInstance, shouldIntegrateInstance } from '$lib/services/schema-authoring';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { pinReachableNodes } from '$lib/utils/pin-node-reachability';
  import { navigationStore, updateTabContent } from '$lib/stores/navigation.svelte';
  import TableView from '$lib/components/query/table-view.svelte';
  import ListView from '$lib/components/query/list-view.svelte';
  import KanbanView from '$lib/components/query/kanban-view.svelte';
  import QueryEditor from '$lib/components/query/query-editor.svelte';
  import type { QueryDefinition } from '$lib/types/query';
  import type { SchemaNode, SchemaField } from '$lib/types/schema-node';
  import type { Node } from '$lib/types';
  import { createLogger } from '$lib/utils/logger';
  import {
    DEFAULT_QUERY_TITLE,
    MATERIALIZED_QUERY_TITLE,
    resolveViewerMode,
    parseQueryDefinition,
    parseViewConfig,
    mergeViewConfig,
    buildMaterializedProperties,
    executeQueryDefinition,
    shouldShowCreatedNode,
    unevaluableFilters,
    type QueryViewKind,
    type QueryViewConfigState,
    type ViewerMode
  } from '$lib/components/query/query-node-model';

  const log = createLogger('QueryNodeViewer');

  // Upper bound on the candidate set fetched before client-side filtering.
  // queryNodes only filters by nodeType, so a saved query must fetch its type's
  // nodes and filter them here; without an explicit limit the daemon caps at 100,
  // which would filter an arbitrary first-100 slice. This bounds the fetch while
  // covering realistic type sizes; larger sets surface a "capped" caveat.
  const FETCH_LIMIT = 1000;

  let {
    nodeId,
    paneId,
    onNodeIdChange
  }: {
    nodeId: string;
    paneId?: string;
    /** Re-point this tab at a new node id (used after materialization). Supplied
     *  by pane-content; a navigation-store fallback covers its absence. */
    onNodeIdChange?: (_newNodeId: string) => void;
  } = $props();

  /** Which shape we're serving; set once the backing node is loaded. */
  let mode = $state<ViewerMode>('default');
  /** The saved query node (SAVED branch only); null on the default type view. */
  let queryNode = $state<Node | null>(null);
  /** The schema whose fields drive columns / Kanban grouping. */
  let schemaNode = $state<SchemaNode | null>(null);
  /** The node type the query targets — schema id (default) or the query's
   *  stored targetType (saved). Inherited, never asked for. */
  let targetType = $state('');

  // IDs of nodes loaded for this view.
  // TableView calls sharedNodeStore.getNode(id) per row inside its reactive template,
  // which is how task-node.svelte achieves live reactivity — the lookup happens inside
  // the Svelte component's tracked context, not in a pre-computed $derived array.
  let loadedNodeIds = $state<string[]>([]);
  // Query results routinely live under UNRELATED parent documents — they are
  // not structureTree children of this query node — so SharedNodeStore's
  // structureTree-walk eviction check can't see them as reachable on their
  // own. Pin the current result set explicitly for as long as this viewer
  // displays it; without this, a row could be silently evicted out from
  // under a still-open view after the inactivity threshold, indistinguishable
  // from the node having been deleted (see table-row.svelte's `nodeExists`).
  const pinOwnerId = uuidv4();
  $effect(() => pinReachableNodes(pinOwnerId, loadedNodeIds));
  let queryState = $state<'idle' | 'loading' | 'success' | 'error'>('idle');
  let error = $state<string | null>(null);
  // Sentinel to discard in-flight responses when nodeId changes rapidly (sidenav navigation)
  let currentLoadId = $state(0);

  // Edit mode state
  let isEditMode = $state(false);
  /** Error message shown to user when save/materialize fails */
  let saveError = $state<string | null>(null);
  /** Error message shown to user when creating a new instance fails */
  let createError = $state<string | null>(null);
  /** True while a new-instance create is in flight (disables the button) */
  let isCreating = $state(false);
  /** Guards against a second materialize firing before the tab re-routes. */
  let materializing = $state(false);

  // View state. On the SAVED branch these are restored from the query node's
  // viewConfig; on the DEFAULT branch they are in-memory only — changing either
  // is a divergence that materializes a node.
  let activeView = $state<QueryViewKind>('table');
  let kanbanGroupBy = $state<string | undefined>(undefined);

  // Title editing state
  let isEditingTitle = $state(false);
  let titleDraft = $state('');
  // Set when the user presses Escape so the input's blur-triggered commit is a
  // no-op rather than an unwanted rename/materialize.
  let titleEditCancelled = $state(false);

  // True when the type's node set was capped by FETCH_LIMIT — client-side
  // filtering then ran over a partial set, so the viewer says so.
  let fetchCapped = $state(false);

  const hasResults = $derived(loadedNodeIds.length > 0);

  /** Header title: "Default" for the type view, the query's name when saved. */
  const displayTitle = $derived(
    mode === 'saved'
      ? queryNode?.content?.trim() || MATERIALIZED_QUERY_TITLE
      : DEFAULT_QUERY_TITLE
  );

  /** The definition seeding the filter editor / a materialize. */
  const currentDefinition = $derived.by((): QueryDefinition => {
    if (mode === 'saved' && queryNode) return parseQueryDefinition(queryNode);
    return { targetType, filters: [] };
  });

  /** The view config reflecting the current in-memory selection. */
  const currentViewConfig = $derived.by((): QueryViewConfigState => ({
    lastView: activeView,
    ...(kanbanGroupBy ? { kanban: { groupBy: kanbanGroupBy } } : {})
  }));

  /**
   * A visible note when client-side execution can't be fully faithful — the
   * fetch was capped, or the saved definition has filters that can't be
   * evaluated here (parent/children relationships need graph traversal). Keeps
   * the result honest rather than silently under- or over-returning.
   */
  const executionCaveat = $derived.by((): string | null => {
    const notes: string[] = [];
    if (fetchCapped) notes.push(`showing the first ${FETCH_LIMIT} nodes of this type`);
    if (mode === 'saved' && unevaluableFilters(currentDefinition.filters).length > 0) {
      notes.push(
        'relationship filters (parent/children) can’t be applied here, so results may be broader than the saved query'
      );
    }
    return notes.length > 0 ? notes.join('; ') : null;
  });

  // Load the backing node and execute the query on mount. pane-content remounts
  // this viewer via {#key ...nodeId} when the tab's nodeId changes, so this is a
  // discrete per-node lifecycle load — not a reactive-state watch (ADR-049).
  onMount(() => {
    loadAndQuery(nodeId);

    // Fold in nodes created OUTSIDE this viewer (CLI, an agent tool call, another
    // tab/window) without a remount. The daemon's `node:created` event is hydrated
    // into `sharedNodeStore` by tauri-sync-listener (which already drops events from
    // a non-active database), so a wildcard subscription surfaces every active-DB
    // node change; we append the ones that belong to this view.
    const unsubscribe = sharedNodeStore.subscribeAll((node) => {
      // `shouldShowCreatedNode` gates on the settled view, dedup, type, and the
      // query's filters. Gating on 'success' matters: during a (re)load
      // `loadedNodeIds` is reset and repopulated wholesale and the load's own
      // `setNode` calls fire this callback, so it avoids O(n^2) self-appends and
      // keeps a node arriving mid-switch out of a stale view (the fresh query
      // reconciles it).
      if (shouldShowCreatedNode(node, { queryState, targetType, loadedNodeIds, definition: currentDefinition })) {
        loadedNodeIds = [...loadedNodeIds, node.id];
      }
    });
    return unsubscribe;
  });

  async function safeGetSchema(typeId: string): Promise<SchemaNode | null> {
    try {
      return await backendAdapter.getSchema(typeId);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      if (isSchemaNotFound(message)) return null;
      throw e;
    }
  }

  async function loadAndQuery(id: string) {
    const loadId = ++currentLoadId;
    // ADR-053: capture the database generation so a switch mid-load drops these
    // rows instead of writing the previous database's nodes into the now-active store.
    const epoch = sharedNodeStore.currentEpoch();
    queryState = 'loading';
    error = null;
    saveError = null;
    schemaNode = null;
    queryNode = null;
    loadedNodeIds = [];
    fetchCapped = false;

    try {
      // Load the raw node and branch on what it actually is — the tab's
      // decorative `nodeType: 'query'` is not trusted.
      //
      // Prefer an already-hydrated copy from sharedNodeStore over a fresh
      // network round-trip: this is the remount path immediately after
      // materializeQuery — rerouteTab already seeded the store with the
      // freshly created node synchronously before the {#key} in pane-content
      // swapped nodeId, so it is present here on this instance's very first
      // load, with no dependency on the backend's create→get round-trip
      // having settled. Without this, a `getNode` that raced ahead of the
      // create would resolve this fresh mount to the DEFAULT branch —
      // resetting activeView/kanbanGroupBy to their defaults — even though a
      // real query node now exists with the view the user just chose.
      const raw = sharedNodeStore.getNode(id) ?? (await backendAdapter.getNode(id));
      if (loadId !== currentLoadId) return;
      mode = resolveViewerMode(raw);

      if (mode === 'saved' && raw) {
        // SAVED branch: read the stored definition + view config off the node.
        queryNode = raw;
        const definition = parseQueryDefinition(raw);
        targetType = definition.targetType;
        const viewConfig = parseViewConfig(raw);
        activeView = viewConfig.lastView;
        kanbanGroupBy = viewConfig.kanban?.groupBy;

        // Load the target type's schema for column / Kanban derivation. Tolerate
        // a missing schema (e.g. an AI query with targetType '*') — columns then
        // fall back to generic rendering.
        schemaNode = await safeGetSchema(targetType);
        if (loadId !== currentLoadId) return;

        const nodes = await backendAdapter.queryNodes({ nodeType: targetType, limit: FETCH_LIMIT });
        if (loadId !== currentLoadId) return;
        if (sharedNodeStore.currentEpoch() !== epoch) return;
        fetchCapped = nodes.length >= FETCH_LIMIT;
        const databaseSource = { type: 'database' as const, reason: 'query-node-viewer saved query' };
        // Hydrate the fetched (unfiltered-by-property) set, then execute the
        // definition client-side — queryNodes only filters by nodeType.
        for (const node of nodes) sharedNodeStore.setNode(node, databaseSource);
        loadedNodeIds = executeQueryDefinition(nodes, definition).map((n) => n.id);
        queryState = 'success';
        log.debug('Saved query executed', { nodeId: id, targetType, count: loadedNodeIds.length });
        return;
      }

      // DEFAULT branch: unfiltered fetch of all nodes of the schema's type, no
      // persistence. View state resets to defaults (the default view has no node
      // to restore config from).
      activeView = 'table';
      kanbanGroupBy = undefined;
      const schema = await backendAdapter.getSchema(id);
      if (loadId !== currentLoadId) return;
      schemaNode = schema;
      targetType = schema.id;
      log.debug('Loaded schema node (default view)', { schemaId: id, content: schema.content });

      const nodes = await backendAdapter.queryNodes({ nodeType: schema.id, limit: FETCH_LIMIT });
      if (loadId !== currentLoadId) return;
      if (sharedNodeStore.currentEpoch() !== epoch) return;
      fetchCapped = nodes.length >= FETCH_LIMIT;
      const databaseSource = { type: 'database' as const, reason: 'query-node-viewer default view' };
      for (const node of nodes) sharedNodeStore.setNode(node, databaseSource);
      loadedNodeIds = nodes.map((n) => n.id);
      queryState = 'success';
      log.debug('Default view loaded', { schemaId: schema.id, count: nodes.length });
    } catch (e) {
      if (loadId !== currentLoadId) return;
      const message = e instanceof Error ? e.message : String(e);

      // Schema not found on a fresh database is expected — show empty state, not error
      if (isSchemaNotFound(message)) {
        log.debug('Schema not yet created, showing empty state', { id });
        queryState = 'success';
        return;
      }

      log.error('Failed to load query view', { id, error: message });
      error = message;
      queryState = 'error';
    }
  }

  function isSchemaNotFound(message: string): boolean {
    // Tauri CommandError: "Schema '<id>' not found" (code: SCHEMA_NOT_FOUND)
    // HTTP adapter: parsed from ApiError with code SCHEMA_NOT_FOUND
    return /Schema '.*' not found/.test(message) || message.includes('SCHEMA_NOT_FOUND');
  }

  // Build a lookup map from schema fields for enum label resolution.
  // Deliberately unfiltered: TableView owns column visibility (it drops
  // system-protected fields — see isUserVisibleField), and this map is only ever
  // read by the column names TableView actually renders. Filtering here would be
  // a no-op that gives the visibility rule a second home.
  const fieldSchemaMap = $derived.by(() => {
    const map = new Map<string, SchemaField>();
    if (schemaNode?.fields) {
      for (const f of schemaNode.fields) map.set(f.name, f);
    }
    return map;
  });

  /**
   * Re-point the current tab at a freshly materialized query node. Prefers the
   * pane-supplied callback (which preserves the tab's nodeType) and falls back to
   * the navigation store, matching the tab by this viewer's nodeId + pane.
   */
  function rerouteTab(newNodeId: string): void {
    if (onNodeIdChange) {
      onNodeIdChange(newNodeId);
      return;
    }
    const tab = navigationStore.state.tabs.find(
      (t) => t.content?.nodeId === nodeId && (paneId ? t.paneId === paneId : true)
    );
    if (tab) {
      updateTabContent(tab.id, { nodeId: newNodeId, nodeType: 'query' });
    } else {
      log.warn('materialize: could not find tab to re-route', { nodeId });
    }
  }

  /**
   * Materialize a `nodeType: 'query'` node from the current DEFAULT view and
   * re-route the tab to it. Called on the first divergence (filter edit, view /
   * group-by change, or title rename). `targetType` and `generatedBy: 'user'`
   * are fixed by the model; `content` defaults to "Untitled Query".
   */
  async function materializeQuery(opts: {
    content?: string;
    definition?: QueryDefinition;
    viewConfig?: QueryViewConfigState;
  }): Promise<void> {
    if (materializing) return;
    if (!schemaNode) {
      log.warn('QueryNodeViewer: cannot materialize — schema not loaded');
      return;
    }
    materializing = true;
    saveError = null;
    const epoch = sharedNodeStore.currentEpoch();
    try {
      const properties = buildMaterializedProperties({
        targetType,
        definition: opts.definition ?? currentDefinition,
        viewConfig: opts.viewConfig ?? currentViewConfig
      });
      const newId = uuidv4();
      const content = opts.content ?? MATERIALIZED_QUERY_TITLE;
      await backendAdapter.createNode({
        id: newId,
        nodeType: 'query',
        content,
        properties,
        mentions: [],
        parentId: null
      });
      // ADR-053: a database switch mid-create must not re-route the now-active tab.
      if (sharedNodeStore.currentEpoch() !== epoch) return;
      const created = await backendAdapter.getNode(newId);
      if (sharedNodeStore.currentEpoch() !== epoch) return;
      // Seed the store if we could read it back (so the tab title resolves before
      // the remount). The node was created regardless, so re-route even if the
      // read momentarily returns null — the remount's loadAndQuery(newId) reloads
      // it, and NOT re-routing here would leave an orphan and let the next
      // divergence materialize a duplicate.
      if (created) {
        sharedNodeStore.setNode(created, { type: 'database', reason: 'query-node-viewer materialize' });
      }
      log.debug('Materialized query node from default view', { newId, targetType });
      rerouteTab(newId);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      log.error('QueryNodeViewer: failed to materialize query node', { targetType, error: message });
      saveError = `Failed to save query: ${message}`;
    } finally {
      materializing = false;
    }
  }

  /** Persist a view-config change onto the saved query node. */
  async function persistViewConfig(partial: Partial<QueryViewConfigState>): Promise<void> {
    if (!queryNode) return;
    saveError = null;
    const merged = mergeViewConfig(parseViewConfig(queryNode), partial);
    try {
      const updated = await backendAdapter.updateNode(queryNode.id, queryNode.version, {
        properties: { ...queryNode.properties, viewConfig: merged }
      });
      queryNode = updated;
      sharedNodeStore.setNode(updated, { type: 'database', reason: 'query-node-viewer view config' });
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      log.error('QueryNodeViewer: failed to persist view config', { error: message });
      saveError = `Failed to save view: ${message}`;
    }
  }

  async function handleQuerySave(definition: QueryDefinition): Promise<void> {
    saveError = null;
    // targetType is inherited from the schema and never changed by the editor.
    const filters = definition.filters;
    const sorting = definition.sorting;
    const limit = definition.limit;

    if (mode === 'saved') {
      if (!queryNode) {
        log.warn('QueryNodeViewer: cannot save — query node not loaded');
        return;
      }
      try {
        const updated = await backendAdapter.updateNode(queryNode.id, queryNode.version, {
          properties: { ...queryNode.properties, targetType, filters, sorting, limit }
        });
        queryNode = updated;
        sharedNodeStore.setNode(updated, { type: 'database', reason: 'query-node-viewer save' });
        isEditMode = false;
        log.debug('QueryNodeViewer: query definition saved', { nodeId: updated.id });
        // Re-execute with the updated definition.
        untrack(() => loadAndQuery(nodeId));
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        log.error('QueryNodeViewer: failed to save query definition', { error: message });
        saveError = `Failed to save query: ${message}`;
      }
      return;
    }

    // DEFAULT branch: a filter edit is a divergence — materialize (the remount
    // leaves edit mode behind). On failure the editor stays open with saveError.
    await materializeQuery({ definition: { targetType, filters, sorting, limit } });
  }

  async function handleQueryPreview(definition: QueryDefinition): Promise<number> {
    // queryNodes only filters by nodeType, so apply the definition's filters
    // (and sort/limit) client-side — the same path the saved query executes —
    // otherwise the preview reports the whole-type count regardless of filters.
    const nodes = await backendAdapter.queryNodes({
      nodeType: definition.targetType,
      limit: FETCH_LIMIT,
    });
    return executeQueryDefinition(nodes, definition).length;
  }

  function handleQueryCancel(): void {
    isEditMode = false;
  }

  function handleViewChange(view: QueryViewKind): void {
    if (view === activeView) return; // clicking the active view is not a divergence
    activeView = view;
    if (mode === 'saved') {
      persistViewConfig({ lastView: view });
    } else {
      materializeQuery({
        viewConfig: { lastView: view, ...(kanbanGroupBy ? { kanban: { groupBy: kanbanGroupBy } } : {}) }
      });
    }
  }

  function handleKanbanGroupByChange(groupBy: string): void {
    if (groupBy === kanbanGroupBy) return;
    kanbanGroupBy = groupBy;
    if (mode === 'saved') {
      persistViewConfig({ kanban: { groupBy } });
    } else {
      materializeQuery({ viewConfig: { lastView: activeView, kanban: { groupBy } } });
    }
  }

  function focusOnMount(el: HTMLInputElement): void {
    el.focus();
    el.select();
  }

  function startEditTitle(): void {
    // The default's placeholder "Default" is not a real name — start from empty.
    titleDraft = mode === 'saved' ? (queryNode?.content ?? '') : '';
    titleEditCancelled = false;
    isEditingTitle = true;
  }

  async function commitTitle(): Promise<void> {
    // Escape cancels the edit; the input's blur still fires commitTitle, so this
    // guard keeps a cancelled edit from renaming/materializing with the draft.
    if (titleEditCancelled) {
      titleEditCancelled = false;
      isEditingTitle = false;
      return;
    }
    const name = titleDraft.trim();
    isEditingTitle = false;
    saveError = null;

    if (mode === 'saved') {
      if (!queryNode || !name || name === queryNode.content) return;
      try {
        const updated = await backendAdapter.updateNode(queryNode.id, queryNode.version, {
          content: name
        });
        queryNode = updated;
        sharedNodeStore.setNode(updated, { type: 'database', reason: 'query-node-viewer rename' });
        log.debug('QueryNodeViewer: query renamed', { nodeId: updated.id });
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        log.error('QueryNodeViewer: failed to rename query', { error: message });
        saveError = `Failed to rename query: ${message}`;
      }
      return;
    }

    // DEFAULT branch: naming the default is a divergence — materialize with the
    // typed name. An empty name leaves the default in place (no materialize).
    if (name) await materializeQuery({ content: name });
  }

  function handleTitleKeydown(e: KeyboardEvent): void {
    if (e.key === 'Enter') {
      e.preventDefault();
      (e.currentTarget as HTMLInputElement).blur();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      titleEditCancelled = true;
      isEditingTitle = false;
    }
  }

  function handleRowClick(clickedNodeId: string) {
    // Focus the node if it's already open; otherwise open it in the other pane
    // so this result list stays visible alongside it.
    const nav = getNavigationService();
    if (nav.focusNodeTab(clickedNodeId)) return;
    nav.navigateToNodeInOtherPane(clickedNodeId, paneId);
  }

  // Create a fresh instance of the viewed schema type. The list is populated by
  // queryNodes({ nodeType: schemaNode.id }), so the new node is minted with that
  // same nodeType to appear in the current results; content/fields are left empty
  // for the schema-driven form UI to fill in once the node is opened.
  async function handleCreateInstance(): Promise<void> {
    if (!schemaNode || isCreating) return;
    const typeId = schemaNode.id;
    // Capture the load generation + database epoch so a mid-flight database
    // switch or re-query drops the new node instead of injecting it into a
    // now-stale view — the same ADR-053 discipline loadAndQuery applies.
    const captured = { loadId: currentLoadId, epoch: sharedNodeStore.currentEpoch() };
    isCreating = true;
    createError = null;
    try {
      const created = await createSchemaInstance(typeId);
      const current = { loadId: currentLoadId, epoch: sharedNodeStore.currentEpoch() };
      if (!shouldIntegrateInstance(captured, current)) {
        log.debug('QueryNodeViewer: discarding new instance — view changed mid-create', {
          typeId,
          newId: created.id,
        });
        return;
      }
      // Hydrate the new node into the shared store so the TableView row lookup
      // resolves it, then append its id so it shows without a full re-query.
      // `setNode` fires the wildcard subscription above synchronously, which may
      // already have appended this id (a default type view matches it), so guard
      // the append against a duplicate — a duplicate key crashes the keyed #each
      // in every view. (A saved query whose filters the new node fails is NOT
      // appended by the subscription, so this append still shows it, as intended.)
      sharedNodeStore.setNode(created, {
        type: 'database',
        reason: 'query-node-viewer new instance',
      });
      if (!loadedNodeIds.includes(created.id)) {
        loadedNodeIds = [...loadedNodeIds, created.id];
      }
      log.debug('QueryNodeViewer: created new instance', { typeId, newId: created.id });
      // Open the new instance immediately for editing, reusing row-open behaviour.
      handleRowClick(created.id);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      log.error('QueryNodeViewer: failed to create new instance', { typeId, error: message });
      createError = `Failed to create new ${schemaNode?.content ?? 'instance'}: ${message}`;
    } finally {
      isCreating = false;
    }
  }
</script>

<div class="query-node-viewer">
  <header class="query-header">
    {#if isEditingTitle}
      <input
        class="query-title-input"
        bind:value={titleDraft}
        onkeydown={handleTitleKeydown}
        onblur={commitTitle}
        placeholder={displayTitle}
        aria-label="Query name"
        use:focusOnMount
      />
    {:else}
      <button
        class="query-title"
        onclick={startEditTitle}
        title="Rename query"
        aria-label={`Query name: ${displayTitle}. Click to rename.`}
      >{displayTitle}</button>
    {/if}
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
    {#if schemaNode && queryState === 'success' && !isEditMode}
      <button
        class="new-instance-button"
        onclick={handleCreateInstance}
        disabled={isCreating}
      >+ New</button>
    {/if}
    {#if schemaNode && queryState === 'success' && !isEditMode}
      <button class="edit-query-button" onclick={() => { isEditMode = true; }}>Edit Query</button>
    {/if}
  </header>

  {#if createError}
    <p class="create-error" role="alert">{createError}</p>
  {/if}

  {#if saveError && !isEditMode}
    <!-- Surface materialize/rename/view-change failures outside the filter editor,
         where the in-editor save-error banner (below) is not visible. -->
    <p class="create-error" role="alert">{saveError}</p>
  {/if}

  {#if queryState === 'success' && executionCaveat}
    <p class="query-caveat" role="status">{executionCaveat}</p>
  {/if}

  {#if isEditMode}
    <div class="edit-mode-wrapper">
      {#if saveError}
        <p class="save-error" role="alert">{saveError}</p>
      {/if}
      <QueryEditor
        query={currentDefinition}
        fields={schemaNode?.fields ?? []}
        {targetType}
        onSave={handleQuerySave}
        onCancel={handleQueryCancel}
        onPreview={handleQueryPreview}
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
    {:else if queryState === 'success' && !hasResults}
      <div class="empty-state">
        <p>{mode === 'saved' ? 'No results match this query.' : 'No nodes of this type yet.'}</p>
      </div>
    {:else if queryState === 'success' && activeView === 'list'}
      <ListView nodeIds={loadedNodeIds} onRowClick={handleRowClick} />
    {:else if queryState === 'success' && activeView === 'kanban'}
      <KanbanView
        nodeIds={loadedNodeIds}
        schema={schemaNode}
        groupBy={kanbanGroupBy}
        onGroupByChange={handleKanbanGroupByChange}
        onRowClick={handleRowClick}
      />
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

  .query-title {
    font-size: 1.5rem;
    font-weight: 600;
    margin: 0;
    color: hsl(var(--foreground));
    flex: 1;
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 0.375rem;
    padding: 0.125rem 0.375rem;
    cursor: text;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    transition: background-color 0.15s ease, border-color 0.15s ease;
  }

  .query-title:hover {
    background: hsl(var(--muted) / 0.6);
  }

  .query-title-input {
    font-size: 1.5rem;
    font-weight: 600;
    margin: 0;
    color: hsl(var(--foreground));
    flex: 1;
    min-width: 0;
    background: hsl(var(--background));
    border: 1px solid hsl(var(--primary));
    border-radius: 0.375rem;
    padding: 0.125rem 0.375rem;
    outline: none;
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

  .new-instance-button {
    padding: 0.25rem 0.625rem;
    font-size: 0.8125rem;
    font-weight: 500;
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
    border: 1px solid hsl(var(--primary));
    border-radius: 0.375rem;
    cursor: pointer;
    transition: opacity 0.15s ease;
    flex-shrink: 0;
  }

  .new-instance-button:hover:not(:disabled) {
    opacity: 0.9;
  }

  .new-instance-button:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .create-error {
    margin: 0;
    padding: 0.5rem 2rem;
    font-size: 0.8125rem;
    color: hsl(var(--destructive));
    background: hsl(var(--destructive) / 0.1);
    border-bottom: 1px solid hsl(var(--destructive) / 0.3);
  }

  .query-caveat {
    margin: 0;
    padding: 0.375rem 2rem;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted) / 0.5);
    border-bottom: 1px solid hsl(var(--border));
  }

  .edit-mode-wrapper {
    padding: 1rem 2rem;
    border-bottom: 1px solid hsl(var(--border));
  }

  .save-error {
    margin: 0 0 0.75rem;
    font-size: 0.8125rem;
    color: hsl(var(--destructive));
    padding: 0.5rem 0.75rem;
    background: hsl(var(--destructive) / 0.1);
    border: 1px solid hsl(var(--destructive) / 0.3);
    border-radius: 0.375rem;
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

</style>
