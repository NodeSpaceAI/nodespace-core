<!--
  NodeTree Component
  
  Provides hierarchical display patterns for parent-child node relationships.
  Supports tree indentation, expand/collapse functionality, and depth tracking.
-->

<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import TextNode from './TextNode.svelte';
  import type { TreeNodeData } from '$lib/types/tree';

  // Props
  export let nodes: TreeNodeData[] = [];
  export const maxDepth: number = 10;
  export let indentSize: number = 4; // Uses --ns-spacing-4 by default
  export const expandedByDefault: boolean = true;
  export let showExpandControls: boolean = true;

  // Event dispatcher
  const dispatch = createEventDispatcher<{
    nodeSelect: { nodeId: string; node: TreeNodeData };
    nodeExpand: { nodeId: string; expanded: boolean };
    nodeUpdate: { nodeId: string; content: string };
    nodeDelete: { nodeId: string };
    nodeMove: { nodeId: string; newParentId: string | null; newDepth: number };
  }>();

  // Toggle node expansion
  function toggleExpansion(nodeId: string) {
    const node = findNodeById(nodeId, nodes);
    if (node && node.hasChildren) {
      node.expanded = !node.expanded;
      dispatch('nodeExpand', { nodeId, expanded: node.expanded });
      // Trigger reactive update
      nodes = [...nodes];
    }
  }

  // Find node by ID (recursive search)
  function findNodeById(nodeId: string, nodeList: TreeNodeData[]): TreeNodeData | null {
    for (const node of nodeList) {
      if (node.id === nodeId) return node;
      const found = findNodeById(nodeId, node.children);
      if (found) return found;
    }
    return null;
  }

  // Get all visible nodes (flattened tree respecting expansion state)
  function getVisibleNodes(nodeList: TreeNodeData[]): TreeNodeData[] {
    const result: TreeNodeData[] = [];

    function addVisibleChildren(nodes: TreeNodeData[]) {
      for (const node of nodes) {
        result.push(node);
        if (node.expanded && node.children.length > 0) {
          addVisibleChildren(node.children);
        }
      }
    }

    // Start with root nodes (depth 0)
    const rootNodes = nodeList.filter((node) => node.depth === 0);
    addVisibleChildren(rootNodes);

    return result;
  }

  // Handle node events
  function handleNodeSave(event: CustomEvent<{ nodeId: string; content: string }>) {
    dispatch('nodeUpdate', event.detail);
  }

  function handleNodeSelect(nodeId: string) {
    const node = findNodeById(nodeId, nodes);
    if (node) {
      dispatch('nodeSelect', { nodeId, node });
    }
  }

  // Get expand/collapse button content
  function getExpandIcon(node: TreeNodeData): string {
    if (!node.hasChildren) return '';
    return node.expanded ? '▼' : '▶';
  }

  // Reactive statement to get visible nodes
  $: visibleNodes = getVisibleNodes(nodes);
</script>

<div class="ns-node-tree" role="tree" aria-label="Node hierarchy">
  {#each visibleNodes as node (node.id)}
    <div
      class="ns-node-tree__item"
      style="margin-left: calc(var(--ns-spacing-{indentSize}) * {node.depth})"
      role="treeitem"
      aria-level={node.depth + 1}
      aria-expanded={node.hasChildren ? node.expanded : undefined}
      aria-selected="false"
    >
      <!-- Expand/collapse control -->
      {#if showExpandControls && node.hasChildren}
        <button
          type="button"
          class="ns-node-tree__expand-btn"
          on:click={() => toggleExpansion(node.id)}
          aria-label="{node.expanded ? 'Collapse' : 'Expand'} {node.title || 'Untitled node'}"
        >
          <span class="ns-node-tree__expand-icon">
            {getExpandIcon(node)}
          </span>
        </button>
      {:else}
        <div class="ns-node-tree__expand-spacer"></div>
      {/if}

      <!-- Node content -->
      <div class="ns-node-tree__content">
        {#if node.nodeType === 'text'}
          <TextNode
            nodeId={node.id}
            content={node.content}
            on:save={handleNodeSave}
            on:click={() => handleNodeSelect(node.id)}
          />
        {:else}
          <!-- Placeholder for other node types -->
          <div class="ns-node-tree__placeholder">
            <p>Node type '{node.nodeType}' not yet implemented</p>
            <p>ID: {node.id} | Depth: {node.depth}</p>
          </div>
        {/if}
      </div>
    </div>
  {/each}
</div>

<style>
  .ns-node-tree {
    width: 100%;
    min-height: 200px;
    /* Base correction factor - exactly matches BaseNode.svelte for consistent circle/chevron alignment */
    --baseline-correction: -0.06375em;
  }

  .ns-node-tree__item {
    display: flex;
    align-items: flex-start;
    gap: var(--ns-spacing-2);
    margin-bottom: var(--ns-spacing-2);
    transition: margin-left var(--ns-duration-normal) var(--ns-easing-easeInOut);
  }

  .ns-node-tree__expand-btn {
    flex-shrink: 0;
    width: 20px;
    height: 20px;
    /* Perfect alignment: empirical correction to match BaseNode circles exactly */
    margin-top: calc(
      0.25rem + var(--text-visual-center, calc(0.816em + var(--baseline-correction))) + 32px
    );
    padding: 0;
    border: none;
    background: var(--ns-color-surface-panel);
    border-radius: var(--ns-radius-sm);
    color: var(--ns-color-text-secondary);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: all var(--ns-duration-fast) var(--ns-easing-easeInOut);
    font-size: var(--ns-font-size-xs);
  }

  .ns-node-tree__expand-btn:hover {
    background: var(--ns-color-surface-hover);
    color: var(--ns-color-text-primary);
    transform: scale(1.1);
  }

  .ns-node-tree__expand-btn:focus-visible {
    outline: 2px solid var(--ns-color-primary-500);
    outline-offset: 2px;
  }

  .ns-node-tree__expand-spacer {
    flex-shrink: 0;
    width: 20px;
    height: 20px;
    /* Match expand button positioning: anchor to circle indicators for consistent alignment */
    margin-top: calc(
      0.25rem + var(--text-visual-center, calc(0.816em + var(--baseline-correction))) + 32px
    );
  }

  .ns-node-tree__expand-icon {
    display: block;
    line-height: 1;
    transition: transform var(--ns-duration-fast) var(--ns-easing-easeInOut);
  }

  .ns-node-tree__content {
    flex: 1;
    min-width: 0;
  }

  .ns-node-tree__placeholder {
    padding: var(--ns-spacing-3);
    background: var(--ns-color-surface-panel);
    border: 1px dashed var(--ns-color-border-default);
    border-radius: var(--ns-radius-md);
    color: var(--ns-color-text-secondary);
    font-size: var(--ns-font-size-sm);
  }

  .ns-node-tree__placeholder p {
    margin: 0 0 var(--ns-spacing-1) 0;
  }

  .ns-node-tree__placeholder p:last-child {
    margin-bottom: 0;
  }

  /* Responsive adjustments */
  @media (max-width: 640px) {
    .ns-node-tree__item {
      gap: var(--ns-spacing-1);
      margin-bottom: var(--ns-spacing-1);
    }

    .ns-node-tree__expand-btn {
      width: 18px;
      height: 18px;
    }

    .ns-node-tree__expand-spacer {
      width: 18px;
      height: 18px;
    }
  }

  /* Accessibility improvements */
  @media (prefers-reduced-motion: reduce) {
    .ns-node-tree__item {
      transition: none;
    }

    .ns-node-tree__expand-btn,
    .ns-node-tree__expand-icon {
      transition: none;
    }
  }

  /* Header-level text-visual-center definitions - exactly matches BaseNode.svelte for perfect chevron alignment */
  .ns-node-tree :global(.node--h1) {
    --text-visual-center: calc(1.2em + var(--baseline-correction) + 0.053125em);
  }

  .ns-node-tree :global(.node--h2) {
    --text-visual-center: calc(0.975em + var(--baseline-correction) + 0.0542em);
  }

  .ns-node-tree :global(.node--h3) {
    --text-visual-center: calc(0.875em + var(--baseline-correction) + 0.1em);
  }

  .ns-node-tree :global(.node--h4) {
    --text-visual-center: calc(0.7875em + var(--baseline-correction));
  }

  .ns-node-tree :global(.node--h5) {
    --text-visual-center: calc(0.7em + var(--baseline-correction));
  }

  .ns-node-tree :global(.node--h6) {
    --text-visual-center: calc(0.6125em + var(--baseline-correction));
  }

  /* High contrast mode support */
  @media (prefers-contrast: high) {
    .ns-node-tree__expand-btn {
      border: 1px solid var(--ns-color-border-strong);
    }

    .ns-node-tree__placeholder {
      border-color: var(--ns-color-border-strong);
    }
  }
</style>
