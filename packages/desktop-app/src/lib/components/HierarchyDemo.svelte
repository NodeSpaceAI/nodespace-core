<!--
  Hierarchy Demo Component
  
  Demonstrates hierarchical display patterns with NodeTree and TextNode integration.
  Shows tree indentation, expand/collapse, and parent-child relationships.
-->

<script lang="ts">
  import { onMount } from 'svelte';
  import NodeTree from './NodeTree.svelte';
  import type { TreeNodeData } from '$lib/types/tree';
  import { mockTextService, type HierarchicalTextNode } from '$lib/services/mockTextService';

  let hierarchicalNodes: TreeNodeData[] = [];
  let loading = true;
  let error: string | null = null;

  // Convert hierarchical nodes to tree node format
  function convertToTreeNodes(nodes: HierarchicalTextNode[]): TreeNodeData[] {
    const result: TreeNodeData[] = [];

    function processNode(node: HierarchicalTextNode): TreeNodeData {
      return {
        id: node.id,
        title: node.title,
        content: node.content,
        nodeType: node.nodeType,
        depth: node.depth,
        parentId: node.parentId,
        children: node.children.map(processNode),
        expanded: node.expanded,
        hasChildren: node.hasChildren
      };
    }

    for (const node of nodes) {
      result.push(processNode(node));
    }

    return result;
  }

  // Load hierarchical data
  async function loadHierarchicalData() {
    try {
      loading = true;
      error = null;

      const nodes = await mockTextService.getHierarchicalNodes();
      hierarchicalNodes = convertToTreeNodes(nodes);
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to load hierarchical data';
    } finally {
      loading = false;
    }
  }

  // Handle node selection
  function handleNodeSelect() {
    // Could implement focus/selection logic here
  }

  // Handle node expansion
  async function handleNodeExpand(event: CustomEvent<{ nodeId: string; expanded: boolean }>) {
    const { nodeId } = event.detail;

    try {
      await mockTextService.toggleNodeExpansion(nodeId);
    } catch {
      // Silently handle expansion errors
    }
  }

  // Handle node updates
  async function handleNodeUpdate(event: CustomEvent<{ nodeId: string; content: string }>) {
    const { nodeId, content } = event.detail;

    try {
      await mockTextService.saveTextNode(nodeId, content);
      // Reload data to reflect changes
      await loadHierarchicalData();
    } catch {
      // Silently handle update errors
    }
  }

  // Load data on mount
  onMount(() => {
    loadHierarchicalData();
  });

  // Get demo statistics
  $: stats = {
    totalNodes:
      hierarchicalNodes.length +
      hierarchicalNodes.reduce((sum, node) => sum + countDescendants(node), 0),
    rootNodes: hierarchicalNodes.filter((node) => node.depth === 0).length,
    maxDepth: Math.max(...getAllNodes(hierarchicalNodes).map((node) => node.depth), 0)
  };

  function countDescendants(node: TreeNodeData): number {
    return (
      node.children.length +
      node.children.reduce((sum: number, child: TreeNodeData) => sum + countDescendants(child), 0)
    );
  }

  function getAllNodes(nodes: TreeNodeData[]): TreeNodeData[] {
    const result: TreeNodeData[] = [];
    function collect(nodeList: TreeNodeData[]) {
      for (const node of nodeList) {
        result.push(node);
        collect(node.children);
      }
    }
    collect(nodes);
    return result;
  }
</script>

<div class="hierarchy-demo">
  <header class="hierarchy-demo__header">
    <h2 class="hierarchy-demo__title">Hierarchical Display Patterns</h2>
    <p class="hierarchy-demo__description">
      Demonstrates tree indentation, expand/collapse functionality, and parent-child node
      relationships.
    </p>

    {#if !loading && !error}
      <div class="hierarchy-demo__stats">
        <span class="stat">
          <strong>{stats.totalNodes}</strong> total nodes
        </span>
        <span class="stat">
          <strong>{stats.rootNodes}</strong> root nodes
        </span>
        <span class="stat">
          <strong>{stats.maxDepth}</strong> max depth
        </span>
        <button type="button" class="hierarchy-demo__refresh-btn" on:click={loadHierarchicalData}>
          Refresh Data
        </button>
      </div>
    {/if}
  </header>

  <main class="hierarchy-demo__content">
    {#if loading}
      <div class="hierarchy-demo__loading">
        <div class="spinner"></div>
        <p>Loading hierarchical structure...</p>
      </div>
    {:else if error}
      <div class="hierarchy-demo__error">
        <p class="error-message">Error: {error}</p>
        <button type="button" on:click={loadHierarchicalData}> Try Again </button>
      </div>
    {:else if hierarchicalNodes.length === 0}
      <div class="hierarchy-demo__empty">
        <p>No hierarchical nodes found.</p>
        <button type="button" on:click={loadHierarchicalData}> Reload </button>
      </div>
    {:else}
      <div class="hierarchy-demo__tree">
        <NodeTree
          nodes={hierarchicalNodes}
          maxDepth={10}
          indentSize={4}
          expandedByDefault={true}
          showExpandControls={true}
          allowEdit={true}
          on:nodeSelect={handleNodeSelect}
          on:nodeExpand={handleNodeExpand}
          on:nodeUpdate={handleNodeUpdate}
        />
      </div>
    {/if}
  </main>

  <aside class="hierarchy-demo__info">
    <h3>Features Demonstrated:</h3>
    <ul>
      <li>
        <strong>Tree Indentation:</strong> CSS
        <code>margin-left: calc(var(--ns-spacing-4) * depth)</code>
      </li>
      <li><strong>Parent Indicators:</strong> Ring effects when nodes have children</li>
      <li><strong>Expand/Collapse:</strong> Toggle visibility of child nodes</li>
      <li><strong>Depth Tracking:</strong> Automatic depth calculation for proper nesting</li>
      <li><strong>Interactive Editing:</strong> Click-to-edit functionality on all nodes</li>
      <li><strong>Hierarchy Management:</strong> Parent-child relationship tracking</li>
    </ul>
  </aside>
</div>

<style>
  .hierarchy-demo {
    display: flex;
    flex-direction: column;
    gap: var(--ns-spacing-4);
    padding: var(--ns-spacing-4);
    max-width: 1200px;
    margin: 0 auto;
  }

  .hierarchy-demo__header {
    text-align: center;
    border-bottom: 1px solid var(--ns-color-border-default);
    padding-bottom: var(--ns-spacing-4);
  }

  .hierarchy-demo__title {
    margin: 0 0 var(--ns-spacing-2) 0;
    font-size: var(--ns-font-size-2xl);
    font-weight: var(--ns-font-weight-bold);
    color: var(--ns-color-text-primary);
  }

  .hierarchy-demo__description {
    margin: 0 0 var(--ns-spacing-3) 0;
    font-size: var(--ns-font-size-base);
    color: var(--ns-color-text-secondary);
    max-width: 600px;
    margin-left: auto;
    margin-right: auto;
  }

  .hierarchy-demo__stats {
    display: flex;
    justify-content: center;
    align-items: center;
    gap: var(--ns-spacing-4);
    flex-wrap: wrap;
  }

  .stat {
    font-size: var(--ns-font-size-sm);
    color: var(--ns-color-text-tertiary);
  }

  .stat strong {
    color: var(--ns-color-primary-600);
    font-weight: var(--ns-font-weight-semibold);
  }

  .hierarchy-demo__refresh-btn {
    padding: var(--ns-spacing-2) var(--ns-spacing-3);
    border: 1px solid var(--ns-color-border-default);
    background: var(--ns-color-surface-panel);
    color: var(--ns-color-text-secondary);
    border-radius: var(--ns-radius-md);
    cursor: pointer;
    font-size: var(--ns-font-size-sm);
    transition: all var(--ns-duration-fast) var(--ns-easing-easeInOut);
  }

  .hierarchy-demo__refresh-btn:hover {
    background: var(--ns-color-surface-hover);
    border-color: var(--ns-color-primary-500);
    color: var(--ns-color-text-primary);
  }

  .hierarchy-demo__content {
    flex: 1;
    min-height: 400px;
  }

  .hierarchy-demo__loading {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--ns-spacing-3);
    padding: var(--ns-spacing-8);
    color: var(--ns-color-text-secondary);
  }

  .spinner {
    width: 32px;
    height: 32px;
    border: 3px solid var(--ns-color-border-default);
    border-top-color: var(--ns-color-primary-500);
    border-radius: 50%;
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .hierarchy-demo__error,
  .hierarchy-demo__empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--ns-spacing-3);
    padding: var(--ns-spacing-8);
    text-align: center;
  }

  .error-message {
    color: var(--ns-color-error-600);
    font-weight: var(--ns-font-weight-medium);
  }

  .hierarchy-demo__tree {
    background: var(--ns-color-surface-default);
    border: 1px solid var(--ns-color-border-default);
    border-radius: var(--ns-radius-lg);
    padding: var(--ns-spacing-4);
    min-height: 300px;
  }

  .hierarchy-demo__info {
    background: var(--ns-color-surface-panel);
    border: 1px solid var(--ns-color-border-default);
    border-radius: var(--ns-radius-lg);
    padding: var(--ns-spacing-4);
    font-size: var(--ns-font-size-sm);
  }

  .hierarchy-demo__info h3 {
    margin: 0 0 var(--ns-spacing-3) 0;
    font-size: var(--ns-font-size-base);
    font-weight: var(--ns-font-weight-semibold);
    color: var(--ns-color-text-primary);
  }

  .hierarchy-demo__info ul {
    margin: 0;
    padding-left: var(--ns-spacing-4);
    color: var(--ns-color-text-secondary);
  }

  .hierarchy-demo__info li {
    margin-bottom: var(--ns-spacing-2);
    line-height: var(--ns-line-height-relaxed);
  }

  .hierarchy-demo__info code {
    background: var(--ns-color-surface-default);
    padding: var(--ns-spacing-1);
    border-radius: var(--ns-radius-sm);
    font-family: var(--ns-font-family-mono);
    font-size: calc(var(--ns-font-size-sm) * 0.9);
    color: var(--ns-color-primary-700);
  }

  /* Responsive design */
  @media (max-width: 768px) {
    .hierarchy-demo {
      padding: var(--ns-spacing-2);
    }

    .hierarchy-demo__stats {
      gap: var(--ns-spacing-2);
    }

    .hierarchy-demo__tree,
    .hierarchy-demo__info {
      padding: var(--ns-spacing-3);
    }
  }
</style>
