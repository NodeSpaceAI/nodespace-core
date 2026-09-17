<!--
  CollectionSubPanel - Slide-out panel showing collection member nodes

  Displays when a collection is clicked in the navigation sidebar.
  Slides in from the left, adjacent to the sidebar.
-->

<script lang="ts">
  import Icon, { type IconName } from '$lib/design/icons/icon.svelte';
  import { type CollectionMember } from '$lib/stores/collections.svelte';
  import { collectionService } from '$lib/services/collection-service';
  import { createNodeInCollection, searchAddableNodes } from '$lib/services/collection-authoring';
  import { createLogger } from '$lib/utils/logger';
  import type { Node } from '$lib/types';

  const log = createLogger('CollectionSubPanel');

  interface Props {
    open: boolean;
    collectionId: string;
    collectionName: string;
    members: CollectionMember[];
    onClose: () => void;
    onNodeClick: (_nodeId: string, _nodeType: string) => void;
    /** Open the collection's own page (Contents / Collaboration tabs). */
    onOpenCollection: () => void;
    /** Called after a node is created in / added to this collection so the parent
        can reload the member list. */
    onChanged: () => void;
  }

  let {
    open,
    collectionId,
    collectionName,
    members,
    onClose,
    onNodeClick,
    onOpenCollection,
    onChanged
  }: Props = $props();

  // --- Authoring into this collection (mirrors the full collection viewer) ----
  let authorBusy = $state(false);
  let showAddExisting = $state(false);
  let addQuery = $state('');
  let addResults: Node[] = $state([]);
  let addSearching = $state(false);
  let addSearchToken = 0;

  async function handleNewNode() {
    if (authorBusy || !collectionId) return;
    authorBusy = true;
    try {
      const newId = await createNodeInCollection(collectionId);
      onChanged();
      onNodeClick(newId, 'text'); // open the empty node to edit
      log.debug('Created node in collection', { newId, collectionId });
    } catch (err) {
      log.error('Failed to create node in collection', { collectionId, error: err });
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

  // Debounce keystrokes — `searchAddableNodes` is a semantic/embedding query
  // (mirrors search-pane.svelte). The token check still drops a late result.
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
      const results = await searchAddableNodes(addQuery, collectionId, excludeIds);
      if (token !== addSearchToken) return;
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
    if (authorBusy || !collectionId) return;
    authorBusy = true;
    try {
      await collectionService.addNodeToCollection(node.id, collectionId);
      addResults = addResults.filter((n) => n.id !== node.id);
      onChanged();
      log.debug('Added existing node to collection', { nodeId: node.id, collectionId });
    } catch (err) {
      log.error('Failed to add existing node to collection', { nodeId: node.id, error: err });
    } finally {
      authorBusy = false;
    }
  }

  // Map content node types to the closest available icon (see icon.svelte for
  // the full IconName set). Anything unmapped falls back to the generic text glyph.
  function getNodeIcon(nodeType: string | undefined | null): IconName {
    if (!nodeType) return 'text';
    const iconMap: Record<string, IconName> = {
      date: 'calendar',
      task: 'taskIncomplete',
      checkbox: 'taskIncomplete',
      'ai-chat': 'aiSquare',
      prompt: 'aiSquare',
      skill: 'aiSquare',
      query: 'aiSquare',
      text: 'text',
      header: 'text',
      'code-block': 'text',
      'quote-block': 'text',
      'ordered-list': 'text'
    };
    return iconMap[nodeType] ?? 'text';
  }

  // Nicely-cased display labels for known content types; anything else is
  // title-cased from its raw id (e.g. 'my-thing' -> 'My Thing').
  const NODE_TYPE_LABELS: Record<string, string> = {
    text: 'Text',
    header: 'Header',
    task: 'Task',
    checkbox: 'Checkbox',
    'code-block': 'Code',
    'quote-block': 'Quote',
    'ordered-list': 'List',
    'ai-chat': 'AI Chat',
    query: 'Query',
    prompt: 'Prompt',
    skill: 'Skill',
    date: 'Date'
  };

  function humanizeNodeType(nodeType: string | undefined | null): string {
    // A member row can render before its node has hydrated (rapid collection
    // switching, initial sync catch-up), so `nodeType` may be undefined. Guard
    // it — an unguarded `.split` here threw on every reactive re-run, storming
    // the log and churning the sub-panel until the node landed.
    if (!nodeType) return 'Item';
    return (
      NODE_TYPE_LABELS[nodeType] ??
      nodeType
        .split(/[-_]/)
        .filter(Boolean)
        .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
        .join(' ')
    );
  }

  // A muted placeholder for content with no name, so blank rows read as
  // "Untitled text" / "Untitled task" instead of appearing empty/unclickable.
  function fallbackName(nodeType: string | undefined | null): string {
    return `Untitled ${humanizeNodeType(nodeType).toLowerCase()}`;
  }
</script>

<div class="sub-panel" class:open>
  <div class="sub-panel-header">
    <button
      class="sub-panel-title"
      onclick={onOpenCollection}
      title="Open collection (manage members & collaboration)"
    >
      <span class="sub-panel-title-text">{collectionName}</span>
      <Icon name="chevronRight" size={14} />
    </button>
    <button class="close-btn" onclick={onClose} aria-label="Close panel">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M18 6L6 18M6 6l12 12" />
      </svg>
    </button>
  </div>

  <div class="sub-toolbar">
    <button class="sub-toolbar-btn" onclick={handleNewNode} disabled={authorBusy}>
      <Icon name="text" size={13} />
      <span>New node</span>
    </button>
    <button
      class="sub-toolbar-btn"
      class:active={showAddExisting}
      onclick={toggleAddExisting}
      disabled={authorBusy}
    >
      <Icon name="circleRing" size={13} />
      <span>Add existing</span>
    </button>
  </div>

  {#if showAddExisting}
    <div class="sub-add-existing">
      <input
        class="sub-add-search"
        type="text"
        placeholder="Search nodes to add…"
        bind:value={addQuery}
        oninput={runAddSearch}
      />
      {#if addSearching}
        <p class="sub-add-hint">Searching…</p>
      {:else if addResults.length > 0}
        <ul class="sub-add-results">
          {#each addResults as r (r.id)}
            <li>
              <button class="sub-add-result" onclick={() => addExisting(r)} disabled={authorBusy}>
                <Icon name={getNodeIcon(r.nodeType)} size={13} />
                <span class="sub-add-result-name">{r.content || 'Untitled'}</span>
                <span class="sub-add-plus">+</span>
              </button>
            </li>
          {/each}
        </ul>
      {:else if addQuery.trim()}
        <p class="sub-add-hint">No matching nodes.</p>
      {/if}
    </div>
  {/if}

  <ul class="node-list">
    {#each members as member (member.id)}
      {@const trimmedName = member.name.trim()}
      <li>
        <button class="node-item" onclick={() => onNodeClick(member.id, member.nodeType)}>
          <Icon name={getNodeIcon(member.nodeType)} size={16} />
          <span class="node-name" class:node-name--untitled={!trimmedName}>
            {trimmedName || fallbackName(member.nodeType)}
          </span>
          <span class="node-type-tag">{humanizeNodeType(member.nodeType)}</span>
        </button>
      </li>
    {/each}
    {#if members.length === 0}
      <li class="empty-state">No nodes in this collection</li>
    {/if}
  </ul>
</div>

<style>
  .sub-panel {
    position: absolute;
    left: var(--sidebar-width, 240px); /* Adjacent to expanded sidebar */
    top: 0;
    width: var(--sidebar-width, 240px);
    height: 100%;
    background: hsl(var(--sidebar-background));
    border-right: 1px solid hsl(var(--border));
    box-shadow: 2px 0 8px rgba(0, 0, 0, 0.1);
    transform: translateX(-100%);
    opacity: 0;
    z-index: 20;
    display: flex;
    flex-direction: column;
    pointer-events: none;
  }

  .sub-panel.open {
    transform: translateX(0);
    opacity: 1;
    pointer-events: auto;
  }

  .sub-panel-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 1rem;
    border-bottom: 1px solid hsl(var(--border));
    flex-shrink: 0;
  }

  .sub-panel-title {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    min-width: 0;
    font-size: 0.875rem;
    font-weight: 600;
    color: hsl(var(--foreground));
    background: none;
    border: none;
    padding: 0.125rem 0.25rem;
    margin: -0.125rem -0.25rem;
    border-radius: 4px;
    cursor: pointer;
  }

  .sub-panel-title:hover {
    background: hsl(var(--border));
  }

  .sub-panel-title-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    background: none;
    border: none;
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    border-radius: 4px;
    flex-shrink: 0;
  }

  .close-btn:hover {
    background: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  .close-btn svg {
    width: 16px;
    height: 16px;
  }

  .sub-toolbar {
    display: flex;
    gap: 0.35rem;
    padding: 0.5rem 0.75rem 0;
  }
  .sub-toolbar-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.3rem 0.55rem;
    font-size: 0.75rem;
    color: hsl(var(--foreground));
    background: hsl(var(--muted) / 0.4);
    border: 1px solid hsl(var(--border));
    border-radius: 6px;
    cursor: pointer;
  }
  .sub-toolbar-btn:hover:not(:disabled) {
    background: hsl(var(--muted) / 0.7);
  }
  .sub-toolbar-btn.active {
    background: hsl(var(--muted) / 0.9);
    border-color: hsl(var(--muted-foreground) / 0.4);
  }
  .sub-toolbar-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .sub-add-existing {
    padding: 0.5rem 0.75rem 0;
  }
  .sub-add-search {
    width: 100%;
    padding: 0.35rem 0.5rem;
    font-size: 0.8125rem;
    color: hsl(var(--foreground));
    background: hsl(var(--background));
    border: 1px solid hsl(var(--border));
    border-radius: 6px;
    box-sizing: border-box;
  }
  .sub-add-hint {
    margin: 0.4rem 0 0;
    font-size: 0.75rem;
    color: hsl(var(--muted-foreground));
  }
  .sub-add-results {
    list-style: none;
    margin: 0.4rem 0 0;
    padding: 0;
    max-height: 200px;
    overflow-y: auto;
  }
  .sub-add-result {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    width: 100%;
    padding: 0.3rem 0.4rem;
    font-size: 0.8125rem;
    color: hsl(var(--foreground));
    background: transparent;
    border: none;
    border-radius: 5px;
    cursor: pointer;
    text-align: left;
  }
  .sub-add-result:hover:not(:disabled) {
    background: hsl(var(--muted) / 0.6);
  }
  .sub-add-result:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .sub-add-result-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub-add-plus {
    color: hsl(var(--muted-foreground));
    flex-shrink: 0;
  }

  .node-list {
    flex: 1;
    overflow-y: auto;
    padding: 0.5rem 0;
    margin: 0;
    list-style: none;
  }

  .node-item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.5rem 1rem;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
  }

  .node-item:hover {
    background: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  .node-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Muted placeholder styling for blank/untitled content. */
  .node-name--untitled {
    font-style: italic;
    opacity: 0.7;
  }

  /* Small muted type tag so content types are distinguishable at a glance. */
  .node-type-tag {
    flex-shrink: 0;
    font-size: 0.6875rem;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    color: hsl(var(--muted-foreground));
    opacity: 0.65;
  }

  .empty-state {
    padding: 1rem;
    text-align: center;
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
    font-style: italic;
  }
</style>
