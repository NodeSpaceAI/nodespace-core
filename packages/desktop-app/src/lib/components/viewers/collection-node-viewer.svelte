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

  Follows the *NodeViewer pattern (like DateNodeViewer) for page-level viewers.
-->

<script lang="ts">
  import Icon from '$lib/design/icons/icon.svelte';
  import { collectionService } from '$lib/services/collection-service';
  import { collectionsData } from '$lib/stores/collections';
  import type { Node, CollectionNode } from '$lib/types';
  import { tabState, addTab, setActiveTab } from '$lib/stores/navigation';
  import { createLogger } from '$lib/utils/logger';
  import { v4 as uuidv4 } from 'uuid';

  const log = createLogger('CollectionNodeViewer');

  // Props using Svelte 5 runes mode - unified NodeViewerProps interface
  // Note: onNodeIdChange not used (collections don't navigate like dates)
  let {
    nodeId,
    onTitleChange
  }: {
    nodeId: string;
    onNodeIdChange?: (_nodeId: string) => void;
    onTitleChange?: (_title: string) => void;
  } = $props();

  // Local state
  let collection: CollectionNode | null = $state(null);
  let members: Node[] = $state([]);
  let loading = $state(true);
  let error: string | null = $state(null);

  // Load collection and members when nodeId changes
  $effect(() => {
    loadCollectionData(nodeId);
  });

  // Set initial tab title when collection is loaded
  $effect(() => {
    if (collection) {
      onTitleChange?.(collection.content || 'Collection');
    }
  });

  async function loadCollectionData(collectionId: string) {
    loading = true;
    error = null;

    try {
      // Get collection members (this also validates the collection exists)
      const memberNodes = await collectionService.getCollectionMembers(collectionId);
      members = memberNodes;

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
    // Check if node is already open in a tab
    const currentState = $tabState;
    const existingTab = currentState.tabs.find((tab) => tab.content?.nodeId === member.id);

    if (existingTab) {
      setActiveTab(existingTab.id, existingTab.paneId);
    } else {
      // Create new tab
      const targetPaneId = getTargetPaneId();
      addTab(
        {
          id: uuidv4(),
          title: member.content || 'Untitled',
          type: 'node',
          content: { nodeId: member.id, nodeType: member.nodeType },
          closeable: true,
          paneId: targetPaneId
        },
        true
      );
    }
  }

  function getTargetPaneId(): string {
    const currentState = $tabState;
    const paneExists = currentState.panes.some((p) => p.id === currentState.activePaneId);
    if (paneExists) {
      return currentState.activePaneId;
    }
    return currentState.panes[0]?.id ?? 'pane-1';
  }

  async function handleRemoveMember(member: Node) {
    try {
      await collectionService.removeNodeFromCollection(member.id, nodeId);
      // Reload members
      members = await collectionService.getCollectionMembers(nodeId);
      log.debug('Removed member from collection', { memberId: member.id, collectionId: nodeId });
    } catch (err) {
      const message = err instanceof Error ? err.message : 'Failed to remove member';
      log.error('Failed to remove member', { memberId: member.id, error: message });
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
  </div>

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
    {:else if members.length === 0}
      <div class="empty-state">
        <Icon name="circleRing" size={48} color="hsl(var(--muted-foreground))" />
        <p>This collection is empty</p>
        <span class="empty-hint">Add nodes to this collection using the /collection command or by dragging nodes here.</span>
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
  </div>
</div>

<style>
  .collection-node-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: hsl(var(--background));
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
    transition: opacity 0.15s ease;
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
    transition: background-color 0.15s ease;
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
    transition: color 0.15s ease;
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
    transition:
      opacity 0.15s ease,
      color 0.15s ease;
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
