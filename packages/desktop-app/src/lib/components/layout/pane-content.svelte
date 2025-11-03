<script lang="ts">
  import { setContext } from 'svelte';
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';
  import { tabState, updateTabTitle, updateTabContent } from '$lib/stores/navigation.js';
  import { pluginRegistry } from '$lib/plugins/plugin-registry';
  import type { Pane } from '$lib/stores/navigation.js';

  // ✅ Receive the PANE as a prop - each pane instance gets its own pane object
  let { pane }: { pane: Pane } = $props();

  // Set paneId in context so all descendant components can access it
  // This avoids prop threading through all component layers
  setContext('paneId', pane.id);

  // Derive tab state using Svelte 5 $derived
  // KEY FIX: Use pane.id instead of global $tabState.activePaneId
  const tabs = $derived($tabState.tabs);
  const activeTabId = $derived($tabState.activeTabIds[pane.id]); // ✅ Use THIS pane's ID
  const activeTab = $derived(tabs.find((t) => t.id === activeTabId));

  // Track loaded viewer components by nodeType
  let viewerComponents = $state<Map<string, unknown>>(new Map());
  let viewerLoadErrors = $state<Map<string, string>>(new Map());

  // Load viewer for active tab's node type
  $effect(() => {
    const nodeType = activeTab?.content?.nodeType;
    if (nodeType && !viewerComponents.has(nodeType) && !viewerLoadErrors.has(nodeType)) {
      (async () => {
        try {
          const viewer = await pluginRegistry.getViewer(nodeType);
          if (viewer) {
            viewerComponents.set(nodeType, viewer);
            viewerComponents = new Map(viewerComponents); // Trigger reactivity
          }
        } catch (error) {
          const errorMessage =
            error instanceof Error ? error.message : 'Unknown error loading viewer';
          console.error(`[PaneContent] Failed to load viewer for ${nodeType}:`, error);
          viewerLoadErrors.set(nodeType, errorMessage);
          viewerLoadErrors = new Map(viewerLoadErrors); // Trigger reactivity
        }
      })();
    }
  });
</script>

{#if activeTab?.content}
  {@const content = activeTab.content}
  {@const nodeType = content.nodeType ?? 'text'}
  {@const loadError = viewerLoadErrors.get(nodeType)}

  {#if loadError}
    <!-- Plugin loading error -->
    <div class="error-state">
      <h2>Failed to Load Viewer</h2>
      <p>Unable to load the viewer for node type: <strong>{nodeType}</strong></p>
      <p class="error-message">{loadError}</p>
      <p class="help-text">Try refreshing the page or contact support if the problem persists.</p>
    </div>
  {:else}
    <!-- Dynamic viewer routing via plugin registry -->
    <!-- Falls back to BaseNodeViewer if no custom viewer registered -->
    {@const ViewerComponent = (viewerComponents.get(nodeType) ??
      BaseNodeViewer) as typeof BaseNodeViewer}

    <!-- KEY FIX: Use {#key} to force separate component instances per pane+nodeId -->
    <!-- This ensures each pane gets its own BaseNodeViewer instance with isolated state -->
    {#key `${pane.id}-${content.nodeId}`}
      <ViewerComponent
        nodeId={content.nodeId}
        onTitleChange={(title: string) => updateTabTitle(activeTabId, title)}
        onNodeIdChange={(newNodeId: string) =>
          updateTabContent(activeTabId, { nodeId: newNodeId, nodeType: content.nodeType })}
      />
    {/key}
  {/if}
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

  /* Error state */
  .error-state {
    padding: 2rem;
    text-align: center;
    color: hsl(var(--destructive));
  }

  .error-state h2 {
    margin: 0 0 1rem 0;
    font-size: 1.25rem;
    font-weight: 600;
  }

  .error-state p {
    margin: 0.5rem 0;
  }

  .error-state .error-message {
    font-family: monospace;
    font-size: 0.875rem;
    background: hsl(var(--muted));
    padding: 0.5rem 1rem;
    border-radius: 0.375rem;
    display: inline-block;
    max-width: 100%;
    word-break: break-word;
  }

  .error-state .help-text {
    margin-top: 1rem;
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
  }
</style>
