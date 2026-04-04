<script lang="ts">
  import { setContext, untrack } from 'svelte';
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';
  import { tabState, updateTabTitle, updateTabContent, closeTab } from '$lib/stores/navigation.js';
  import { pluginRegistry } from '$lib/plugins/plugin-registry';
  import type { Pane } from '$lib/stores/navigation.js';
  import { createLogger } from '$lib/utils/logger';
  import SettingsPane from '$lib/components/settings/settings-pane.svelte';
  import ChatPanel from '$lib/components/chat/chat-panel.svelte';

  const log = createLogger('PaneContent');

  // ✅ Receive the PANE as a prop - each pane instance gets its own pane object
  let { pane }: { pane: Pane } = $props();

  // Set paneId in context so all descendant components can access it
  // This avoids prop threading through all component layers
  // Use IIFE to capture initial value and avoid Svelte state_referenced_locally warning
  // Context is set once at component creation - this is intentional one-time capture
  const paneId = (() => pane.id)();
  setContext('paneId', paneId);

  // Derive tab state using Svelte 5 $derived
  // KEY FIX: Use pane.id instead of global $tabState.activePaneId
  const tabs = $derived($tabState.tabs);
  const activeTabId = $derived($tabState.activeTabIds[pane.id]); // ✅ Use THIS pane's ID
  const activeTab = $derived(tabs.find((t) => t.id === activeTabId));

  // Track loaded viewer components by nodeType
  let viewerComponents = $state<Map<string, unknown>>(new Map());
  let viewerLoadErrors = $state<Map<string, string>>(new Map());
  let viewerLoading = $state<Set<string>>(new Set());

  // Load viewer when needed - moved to function called from onMount to avoid derived context issues
  async function loadViewerForNodeType(nodeType: string) {
    if (viewerComponents.has(nodeType) || viewerLoadErrors.has(nodeType) || viewerLoading.has(nodeType)) {
      return;
    }

    // Fast path: if no viewer is registered for this type, store the fallback immediately
    // without entering viewerLoading state (avoids a null→BaseNodeViewer transition that
    // would unmount/remount the viewer unnecessarily).
    if (!pluginRegistry.hasViewer(nodeType)) {
      viewerComponents = new Map(viewerComponents.set(nodeType, BaseNodeViewer));
      return;
    }

    viewerLoading = new Set(viewerLoading).add(nodeType);

    try {
      const viewer = await pluginRegistry.getViewer(nodeType);
      // Always store a result (viewer or BaseNodeViewer fallback) so the guard
      // viewerComponents.has(nodeType) fires true on subsequent calls and prevents
      // repeated load attempts that cause mount/unmount loops (issue #967).
      viewerComponents = new Map(viewerComponents.set(nodeType, viewer ?? BaseNodeViewer));
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : 'Unknown error loading viewer';
      log.error(`Failed to load viewer for ${nodeType}:`, error);
      viewerLoadErrors = new Map(viewerLoadErrors.set(nodeType, errorMessage));
    } finally {
      const next = new Set(viewerLoading);
      next.delete(nodeType);
      viewerLoading = next;
    }
  }

  // Derive viewer component for active tab.
  // Returns null while the viewer module is still loading (prevents BaseNodeViewer fallback
  // from rendering with an incompatible nodeId, e.g. a schema id passed to QueryNodeViewer)
  const ViewerComponent = $derived.by(() => {
    const nodeType = activeTab?.content?.nodeType ?? 'text';
    if (viewerLoading.has(nodeType)) return null;
    return (viewerComponents.get(nodeType) ?? BaseNodeViewer) as typeof BaseNodeViewer;
  });

  const loadError = $derived.by(() => {
    const nodeType = activeTab?.content?.nodeType ?? 'text';
    return viewerLoadErrors.get(nodeType);
  });

  const isViewerLoading = $derived.by(() => {
    const nodeType = activeTab?.content?.nodeType ?? 'text';
    return viewerLoading.has(nodeType);
  });

  // Load viewer when active tab changes - use $effect but call async function
  // untrack the call to loadViewerForNodeType so mutations to viewerLoading/viewerComponents
  // inside that function don't re-trigger this effect
  $effect(() => {
    const nodeType = activeTab?.content?.nodeType;
    if (nodeType) {
      untrack(() => loadViewerForNodeType(nodeType));
    }
  });

</script>

{#if activeTab?.type === 'settings'}
  <SettingsPane />
{:else if activeTab?.type === 'chat'}
  <ChatPanel />
{:else if activeTab?.content}
  {@const content = activeTab.content}
  {@const nodeType = content.nodeType ?? 'text'}

  {#if loadError}
    <!-- Plugin loading error -->
    <div class="error-state">
      <h2>Failed to Load Viewer</h2>
      <p>Unable to load the viewer for node type: <strong>{nodeType}</strong></p>
      <p class="error-message">{loadError}</p>
      <p class="help-text">Try refreshing the page or contact support if the problem persists.</p>
    </div>
  {:else if isViewerLoading}
    <!-- Viewer module still loading — don't render BaseNodeViewer as fallback -->
    <div class="loading-state">
      <span>Loading...</span>
    </div>
  {:else}
    <!-- Dynamic viewer routing via plugin registry -->
    <!-- Falls back to BaseNodeViewer if no custom viewer registered -->

    <!-- KEY FIX: Use {#key} to force separate component instances per pane+nodeId -->
    <!-- This ensures each pane gets its own BaseNodeViewer instance with isolated state -->
    {#key `${pane.id}-${content.nodeId}`}
      <ViewerComponent
        nodeId={content.nodeId}
        tabId={activeTabId}
        onTitleChange={(title: string) => updateTabTitle(activeTabId, title)}
        onNodeIdChange={(newNodeId: string) => {
          updateTabContent(activeTabId, { nodeId: newNodeId, nodeType: content.nodeType });
        }}
        onNodeNotFound={() => closeTab(activeTabId)}
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

  /* Loading state - shown while viewer module is being lazy-loaded */
  .loading-state {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: hsl(var(--muted-foreground));
    font-size: 0.875rem;
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
