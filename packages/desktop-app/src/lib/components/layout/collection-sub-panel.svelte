<!--
  CollectionSubPanel - Slide-out panel showing collection member nodes

  Displays when a collection is clicked in the navigation sidebar.
  Slides in from the left, adjacent to the sidebar.
-->

<script lang="ts">
  import Icon from '$lib/design/icons/icon.svelte';
  import type { CollectionMember } from '$lib/stores/collections';

  interface Props {
    open: boolean;
    collectionName: string;
    members: CollectionMember[];
    onClose: () => void;
    onNodeClick: (_nodeId: string, _nodeType: string) => void;
  }

  let { open, collectionName, members, onClose, onNodeClick }: Props = $props();

  function getNodeIcon(nodeType: string): 'calendar' | 'circle' | 'text' {
    const iconMap: Record<string, 'calendar' | 'circle' | 'text'> = {
      date: 'calendar',
      task: 'circle',
      text: 'text'
    };
    return iconMap[nodeType] || 'text';
  }
</script>

<div class="sub-panel" class:open>
  <div class="sub-panel-header">
    <span class="sub-panel-title">{collectionName}</span>
    <button class="close-btn" onclick={onClose} aria-label="Close panel">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
        <path d="M18 6L6 18M6 6l12 12" />
      </svg>
    </button>
  </div>

  <ul class="node-list">
    {#each members as member (member.id)}
      <li>
        <button class="node-item" onclick={() => onNodeClick(member.id, member.nodeType)}>
          <Icon name={getNodeIcon(member.nodeType)} size={16} />
          <span class="node-name">{member.name}</span>
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
    transition:
      transform 250ms ease-out,
      opacity 250ms ease-out;
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
    font-size: 0.875rem;
    font-weight: 600;
    color: hsl(var(--foreground));
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
    transition:
      background-color 0.2s,
      color 0.2s;
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
    transition:
      background-color 0.2s,
      color 0.2s;
  }

  .node-item:hover {
    background: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  .node-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .empty-state {
    padding: 1rem;
    text-align: center;
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
    font-style: italic;
  }
</style>
