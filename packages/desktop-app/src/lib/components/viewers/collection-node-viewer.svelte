<!--
  CollectionNodeViewer - Page-level viewer for displaying collection contents

  Features:
  - Shows collection name and description in header
  - Lists all member nodes belonging to this collection
  - Member count badge
  - Quick actions (add to collection, remove members)
  - Empty state when no members
  - Click member to open in new tab
  - Works with both direct member_of edges and path-based collections

  Named as a *NodeViewer for page-level-viewer consistency, but renders its own
  layout directly rather than wrapping BaseNodeViewer (unlike DateNodeViewer).
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import Icon from '$lib/design/icons/icon.svelte';
  import { collectionService } from '$lib/services/collection-service';
  import { collectionsData, NON_CONTENT_NODE_TYPES } from '$lib/stores/collections.svelte';
  import { createNodeInCollection, searchAddableNodes } from '$lib/services/collection-authoring';
  import type { Node, CollectionNode } from '$lib/types';
  import { getNavigationService } from '$lib/services/navigation-service';
  import { createLogger } from '$lib/utils/logger';
  import { getActiveViewerExtensions } from '$lib/plugins/ui-extensions.svelte';
  import ExtensionOutlet from '$lib/plugins/extension-outlet.svelte';

  // Collection sub-views: the built-in "contents" tab (member nodes, always shown)
  // plus any registry-contributed tabs (e.g. the Pro "Collaboration" tab). The
  // active tab is 'contents' or a contributed tab's id.
  let activeView = $state<string>('contents');

  // Viewer extensions contributed for the current Pro-sync variant. Empty in the
  // community build (variant `teaser`), so the tab strip and the collaboration
  // view never appear — the collection viewer imports nothing Pro.
  const collabExtensions = $derived(getActiveViewerExtensions('collection'));
  const activeExtension = $derived(collabExtensions.find((e) => e.tab.id === activeView));

  const log = createLogger('CollectionNodeViewer');

  // Props using Svelte 5 runes mode - unified NodeViewerProps interface.
  // The tab title is derived from the node's content by tab-system.svelte — no push here.
  let {
    nodeId
  }: {
    nodeId: string;
  } = $props();

  // Local state
  let collection: CollectionNode | null = $state(null);
  let members: Node[] = $state([]);
  let loading = $state(true);
  let error: string | null = $state(null);

  // Load collection and members on mount. pane-content remounts this viewer via
  // {#key ...nodeId} when the tab's nodeId changes, so this is a discrete per-node
  // lifecycle load — not a reactive-state watch (ADR-049).
  onMount(() => {
    loadCollectionData(nodeId);
  });

  async function loadCollectionData(collectionId: string) {
    loading = true;
    error = null;

    try {
      // Get collection members (this also validates the collection exists)
      const memberNodes = await collectionService.getCollectionMembers(collectionId);
      // The Contents tab lists user-authored content only. `getCollectionMembers`
      // also returns non-content members — chiefly the creator's `person` node
      // (stamped as an admin member on collection creation) and system nodes —
      // which belong in the Collaboration tab, not here. Filter them out (mirrors
      // the sidebar sub-panel's `selectedCollectionMembers`).
      members = memberNodes.filter((n) => !NON_CONTENT_NODE_TYPES.has(n.nodeType));

      // Try to get collection details from cached store data first (by ID)
      const cachedCollection = collectionsData.getCollectionById(collectionId);
      if (cachedCollection) {
        // Convert CollectionInfo to CollectionNode format
        collection = {
          id: cachedCollection.id,
          nodeType: 'collection',
          content: cachedCollection.content,
          createdAt: cachedCollection.createdAt,
          modifiedAt: cachedCollection.modifiedAt,
          version: cachedCollection.version,
          properties: cachedCollection.properties as CollectionNode['properties']
        };
      } else {
        // Fallback: Try to get collection by name (legacy behavior)
        // This handles cases where the viewer is opened before sidebar loaded collections
        const collectionByName = await collectionService.getCollectionByName(collectionId);
        if (collectionByName) {
          collection = collectionByName;
        } else {
          // Last resort: Create placeholder with ID as name
          collection = {
            id: collectionId,
            nodeType: 'collection',
            content: collectionId,
            createdAt: new Date().toISOString(),
            modifiedAt: new Date().toISOString(),
            version: 1,
            properties: {}
          };
        }
      }

      log.debug('Loaded collection data', { collectionId, memberCount: members.length });
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to load collection';
      log.error('Failed to load collection', { collectionId, error: message });
      error = message;
    } finally {
      loading = false;
    }
  }

  function getNodeIcon(nodeType: string): 'calendar' | 'circle' | 'text' | 'circleRing' {
    const iconMap: Record<string, 'calendar' | 'circle' | 'text' | 'circleRing'> = {
      date: 'calendar',
      task: 'circle',
      text: 'text',
      collection: 'circleRing'
    };
    return iconMap[nodeType] || 'text';
  }

  function handleMemberClick(member: Node) {
    getNavigationService().focusOrOpenNode(member.id, {
      nodeType: member.nodeType,
      // The member row already carries its text, so the tab can be titled
      // immediately rather than showing "Loading..." until the viewer mounts.
      title: member.content || 'Untitled'
    });
  }

  // Reload the Contents list (user-authored members only — same filter as the
  // initial load), used after any add/remove so the count and rows stay honest.
  async function reloadContentMembers() {
    members = (await collectionService.getCollectionMembers(nodeId)).filter(
      (n) => !NON_CONTENT_NODE_TYPES.has(n.nodeType)
    );
  }

  async function handleRemoveMember(member: Node) {
    try {
      await collectionService.removeNodeFromCollection(member.id, nodeId);
      await reloadContentMembers();
      log.debug('Removed member from collection', { memberId: member.id, collectionId: nodeId });
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to remove member';
      log.error('Failed to remove member', { memberId: member.id, error: message });
    }
  }

  // --- Authoring into this collection -------------------------------------
  // A collection is a `member_of` grouping over outliner nodes, so both actions
  // below ultimately call `add_node_to_collection`. "New node" mints a fresh
  // text node first (and opens it to edit); "Add existing" attaches a node the
  // user already has. `authorBusy` serializes them so a double-click can't
  // create two nodes or fire overlapping reloads.
  let authorBusy = $state(false);
  let showAddExisting = $state(false);
  let addQuery = $state('');
  let addResults: Node[] = $state([]);
  let addSearching = $state(false);
  let addSearchToken = 0;

  async function handleNewNode() {
    if (authorBusy) return;
    authorBusy = true;
    try {
      const newId = await createNodeInCollection(nodeId);
      await reloadContentMembers();
      // Open the empty node so the user can type into it immediately.
      handleMemberClick({ id: newId, nodeType: 'text', content: '' } as Node);
      log.debug('Created node in collection', { newId, collectionId: nodeId });
    } catch (err) {
      // Log only — matches handleRemoveMember. Setting the shared `error` state
      // would trip the full-screen error branch and wipe the whole Contents view.
      log.error('Failed to create node in collection', { collectionId: nodeId, error: err });
    } finally {
      authorBusy = false;
    }
  }

  function toggleAddExisting() {
    showAddExisting = !showAddExisting;
    if (!showAddExisting) {
      addQuery = '';
      addResults = [];
    }
  }

  let addSearchTimer: ReturnType<typeof setTimeout> | undefined;

  // `searchAddableNodes` runs a semantic/embedding query, so debounce keystrokes
  // instead of firing one per character (mirrors search-pane.svelte). The
  // `addSearchToken` check still drops a slow earlier request that lands late.
  function runAddSearch() {
    if (addSearchTimer) clearTimeout(addSearchTimer);
    if (!addQuery.trim()) {
      addResults = [];
      addSearching = false;
      return;
    }
    addSearching = true; // immediate feedback while the debounce settles
    addSearchTimer = setTimeout(doAddSearch, 200);
  }

  async function doAddSearch() {
    const token = ++addSearchToken;
    addSearching = true;
    try {
      const excludeIds = new Set(members.map((m) => m.id));
      const results = await searchAddableNodes(addQuery, nodeId, excludeIds);
      if (token !== addSearchToken) return; // superseded by a newer keystroke
      addResults = results;
    } catch (err) {
      if (token !== addSearchToken) return;
      log.error('search_roots failed for add-existing', err);
      addResults = [];
    } finally {
      if (token === addSearchToken) addSearching = false;
    }
  }

  async function addExisting(node: Node) {
    if (authorBusy) return;
    authorBusy = true;
    try {
      await collectionService.addNodeToCollection(node.id, nodeId);
      await reloadContentMembers();
      addResults = addResults.filter((n) => n.id !== node.id);
      log.debug('Added existing node to collection', { nodeId: node.id, collectionId: nodeId });
    } catch (err) {
      // Log only — do not wipe the Contents view on a transient add failure.
      log.error('Failed to add existing node to collection', { memberId: node.id, error: err });
    } finally {
      authorBusy = false;
    }
  }
</script>

<div class="collection-node-viewer">
  <!-- Header -->
  <div class="collection-header">
    <div class="collection-title">
      <Icon name="circleRing" size={24} color="hsl(var(--muted-foreground))" />
      <h1>{collection?.content || 'Collection'}</h1>
      {#if !loading}
        <span class="member-count">{members.length} {members.length === 1 ? 'item' : 'items'}</span>
      {/if}
    </div>

    {#if collection?.properties?.description}
      <p class="collection-description">{collection.properties.description}</p>
    {/if}

    {#if collabExtensions.length > 0}
      <!-- Registry-contributed tabs (e.g. the Pro "Collaboration" tab) alongside
           the built-in contents view; the strip only appears when something
           contributes for the active variant. -->
      <div class="collection-tabs" role="tablist">
        <button
          class="tab"
          class:active={activeView === 'contents'}
          role="tab"
          aria-selected={activeView === 'contents'}
          onclick={() => (activeView = 'contents')}>Contents</button
        >
        {#each collabExtensions as ext (ext.tab.id)}
          <button
            class="tab"
            class:active={activeView === ext.tab.id}
            role="tab"
            aria-selected={activeView === ext.tab.id}
            onclick={() => (activeView = ext.tab.id)}>{ext.tab.label}</button
          >
        {/each}
      </div>
    {/if}
  </div>

  {#if activeExtension}
    {#key activeExtension.variant}
      <ExtensionOutlet load={activeExtension.lazyLoad} props={{ nodeId }} />
    {/key}
  {:else}
  <!-- Content -->
  <div class="collection-content">
    {#if loading}
      <div class="loading-state">
        <span>Loading collection...</span>
      </div>
    {:else if error}
      <div class="error-state">
        <Icon name="circle" size={24} color="hsl(var(--destructive))" />
        <span>{error}</span>
        <button class="retry-button" onclick={() => loadCollectionData(nodeId)}>
          Try Again
        </button>
      </div>
    {:else}
      <div class="collection-toolbar">
        <button class="toolbar-btn" onclick={handleNewNode} disabled={authorBusy}>
          <Icon name="text" size={14} color="currentColor" />
          <span>New node</span>
        </button>
        <button
          class="toolbar-btn"
          class:active={showAddExisting}
          onclick={toggleAddExisting}
          disabled={authorBusy}
        >
          <Icon name="circleRing" size={14} color="currentColor" />
          <span>Add existing</span>
        </button>
      </div>

      {#if showAddExisting}
        <div class="add-existing">
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="add-search"
            type="text"
            placeholder="Search your nodes to add…"
            bind:value={addQuery}
            oninput={runAddSearch}
            autofocus
          />
          {#if addSearching}
            <p class="add-hint">Searching…</p>
          {:else if addResults.length > 0}
            <ul class="add-results">
              {#each addResults as r (r.id)}
                <li>
                  <button class="add-result" onclick={() => addExisting(r)} disabled={authorBusy}>
                    <Icon name={getNodeIcon(r.nodeType)} size={14} color="currentColor" />
                    <span class="add-result-name">{r.content || 'Untitled'}</span>
                    <span class="add-result-type">{r.nodeType}</span>
                    <span class="add-plus">+ Add</span>
                  </button>
                </li>
              {/each}
            </ul>
          {:else if addQuery.trim()}
            <p class="add-hint">No matching nodes.</p>
          {/if}
        </div>
      {/if}

      {#if members.length === 0}
        <div class="empty-state">
          <Icon name="circleRing" size={48} color="hsl(var(--muted-foreground))" />
          <p>This collection is empty</p>
          <span class="empty-hint"
            >Use “New node” to create content here, or “Add existing” to pull in a node you already
            have.</span
          >
        </div>
      {:else}
      <ul class="member-list">
        {#each members as member (member.id)}
          <li class="member-item">
            <button
              class="member-button"
              onclick={() => handleMemberClick(member)}
              aria-label="Open {member.content || 'node'}"
            >
              <Icon name={getNodeIcon(member.nodeType)} size={16} color="currentColor" />
              <span class="member-name">{member.content || 'Untitled'}</span>
              <span class="member-type">{member.nodeType}</span>
            </button>
            <button
              class="remove-button"
              onclick={() => handleRemoveMember(member)}
              aria-label="Remove from collection"
              title="Remove from collection"
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M18 6L6 18M6 6l12 12" />
              </svg>
            </button>
          </li>
        {/each}
      </ul>
      {/if}
    {/if}
  </div>
  {/if}
</div>

<style>
  .collection-node-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: hsl(var(--background));
  }

  .collection-tabs {
    display: flex;
    gap: 0.25rem;
    margin-top: 1rem;
  }
  .collection-tabs .tab {
    background: transparent;
    border: none;
    border-bottom: 2px solid transparent;
    padding: 0.35rem 0.75rem;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
    cursor: pointer;
  }
  .collection-tabs .tab.active {
    color: hsl(var(--foreground));
    border-bottom-color: hsl(var(--primary));
  }

  .collection-header {
    padding: 1.5rem 2rem;
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
  }

  .collection-title {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .collection-title h1 {
    font-size: 1.5rem;
    font-weight: 600;
    margin: 0;
    color: hsl(var(--foreground));
  }

  .member-count {
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
    padding: 0.25rem 0.5rem;
    background: hsl(var(--muted));
    border-radius: 9999px;
    margin-left: auto;
  }

  .collection-description {
    margin: 0.5rem 0 0 2.25rem;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
  }

  .collection-content {
    flex: 1;
    overflow-y: auto;
    padding: 1rem 2rem;
  }

  .collection-toolbar {
    display: flex;
    gap: 0.5rem;
    margin-bottom: 0.75rem;
  }
  .toolbar-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.35rem 0.7rem;
    font-size: 0.8125rem;
    color: hsl(var(--foreground));
    background: hsl(var(--muted) / 0.4);
    border: 1px solid hsl(var(--border));
    border-radius: 6px;
    cursor: pointer;
  }
  .toolbar-btn:hover:not(:disabled) {
    background: hsl(var(--muted) / 0.7);
  }
  .toolbar-btn.active {
    background: hsl(var(--muted) / 0.9);
    border-color: hsl(var(--muted-foreground) / 0.4);
  }
  .toolbar-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .add-existing {
    margin-bottom: 0.75rem;
    padding: 0.5rem;
    border: 1px solid hsl(var(--border));
    border-radius: 8px;
    background: hsl(var(--muted) / 0.25);
  }
  .add-search {
    width: 100%;
    padding: 0.4rem 0.6rem;
    font-size: 0.875rem;
    color: hsl(var(--foreground));
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    border-radius: 6px;
    box-sizing: border-box;
  }
  .add-hint {
    margin: 0.5rem 0 0.25rem;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
  }
  .add-results {
    list-style: none;
    margin: 0.5rem 0 0;
    padding: 0;
    max-height: 240px;
    overflow-y: auto;
  }
  .add-result {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.4rem 0.5rem;
    font-size: 0.875rem;
    color: hsl(var(--foreground));
    background: transparent;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
  }
  .add-result:hover:not(:disabled) {
    background: hsl(var(--muted) / 0.6);
  }
  .add-result:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .add-result-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .add-result-type {
    font-size: 0.6875rem;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--muted) / 0.6);
    padding: 0.05rem 0.35rem;
    border-radius: 4px;
  }
  .add-plus {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    flex-shrink: 0;
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
  }

  .error-state {
    color: hsl(var(--destructive));
  }

  .error-state span {
    margin-bottom: 1rem;
  }

  .retry-button {
    padding: 0.5rem 1rem;
    font-size: 0.875rem;
    background: hsl(var(--primary));
    color: hsl(var(--primary-foreground));
    border: none;
    border-radius: 0.375rem;
    cursor: pointer;
  }

  .retry-button:hover {
    opacity: 0.9;
  }

  .empty-state p {
    margin: 1rem 0 0.5rem;
    font-size: 1rem;
  }

  .empty-hint {
    font-size: 0.875rem;
    max-width: 300px;
  }

  .member-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .member-item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    border-radius: 0.375rem;
  }

  .member-item:hover {
    background: hsl(var(--muted));
  }

  .member-button {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.75rem 1rem;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    color: hsl(var(--foreground));
    font-size: 0.875rem;
  }

  .member-button:hover {
    color: hsl(var(--primary));
  }

  .member-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .member-type {
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
    padding: 0.125rem 0.375rem;
    background: hsl(var(--muted));
    border-radius: 0.25rem;
    text-transform: capitalize;
  }

  .remove-button {
    width: 24px;
    height: 24px;
    padding: 0;
    margin-right: 0.5rem;
    background: none;
    border: none;
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    opacity: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 0.25rem;
  }

  .member-item:hover .remove-button {
    opacity: 1;
  }

  .remove-button:hover {
    color: hsl(var(--destructive));
    background: hsl(var(--destructive) / 0.1);
  }

  .remove-button svg {
    width: 14px;
    height: 14px;
  }
</style>
