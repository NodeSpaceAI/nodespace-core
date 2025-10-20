<script lang="ts">
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';
  import { toggleTheme } from '$lib/design/theme.js';
  import { tabState, updateTabTitle, updateTabContent } from '$lib/stores/navigation.js';
  import { pluginRegistry } from '$lib/plugins/plugin-registry';

  // Derive tab state using Svelte 5 $derived
  const tabs = $derived($tabState.tabs);
  const activeTabId = $derived($tabState.activeTabId);
  const activeTab = $derived(tabs.find((t) => t.id === activeTabId));

  // Track loaded viewer components by nodeType
  let viewerComponents = $state<Map<string, unknown>>(new Map());

  // Load viewer for active tab's node type
  $effect(() => {
    const nodeType = activeTab?.content?.nodeType;
    if (nodeType && !viewerComponents.has(nodeType)) {
      (async () => {
        const viewer = await pluginRegistry.getViewer(nodeType);
        if (viewer) {
          viewerComponents.set(nodeType, viewer);
          viewerComponents = new Map(viewerComponents); // Trigger reactivity
        }
      })();
    }
  });

  // Global keyboard shortcuts
  function handleKeydown(event: KeyboardEvent) {
    // Toggle theme - Cmd+\ (Mac) or Ctrl+\ (Windows/Linux) per design system
    if ((event.metaKey || event.ctrlKey) && event.key === '\\') {
      event.preventDefault();
      toggleTheme();
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

{#if activeTab?.content}
  <!-- Dynamic viewer routing via plugin registry -->
  <!-- Falls back to BaseNodeViewer if no custom viewer registered -->
  {@const content = activeTab.content}
  {@const ViewerComponent = (viewerComponents.get(content.nodeType ?? 'text') ??
    BaseNodeViewer) as typeof BaseNodeViewer}

  <ViewerComponent
    nodeId={content.nodeId}
    onTitleChange={(title: string) => updateTabTitle(activeTabId, title)}
    onNodeIdChange={(newNodeId: string) =>
      updateTabContent(activeTabId, { nodeId: newNodeId, nodeType: content.nodeType })}
  />
{:else if activeTab}
  <!-- Placeholder content for tabs without node content -->
  <div class="placeholder-content">
    <h2>{activeTab.title}</h2>
    <p>This is a placeholder tab. Content will be implemented later.</p>
  </div>
{:else}
  <!-- No active tab -->
  <div class="empty-state">
    <p>No tab selected</p>
  </div>
{/if}

<style>
  /* Placeholder content */
  .placeholder-content {
    padding: 2rem;
    text-align: center;
  }

  .placeholder-content h2 {
    margin: 0 0 1rem 0;
    color: hsl(var(--foreground));
  }

  .placeholder-content p {
    margin: 0.5rem 0;
    color: hsl(var(--muted-foreground));
  }

  /* Empty state */
  .empty-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: hsl(var(--muted-foreground));
  }
</style>
