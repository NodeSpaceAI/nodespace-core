<script lang="ts">
  import { onMount, setContext, untrack } from 'svelte';
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';
  import { navigationStore, updateTabContent, closeTab } from '$lib/stores/navigation.svelte';
  import { pluginRegistry } from '$lib/plugins/plugin-registry';
  import { sharedNodeStore } from '$lib/services/shared-node-store.svelte';
  import { onDaemonReconnect } from '$lib/services/daemon-status';
  import type { Pane } from '$lib/stores/navigation.svelte';
  import { createLogger } from '$lib/utils/logger';
  import SettingsPane from '$lib/components/settings/settings-pane.svelte';
  import SearchPane from '$lib/components/search/search-pane.svelte';
  import ConflictsPane from '$lib/components/conflicts/conflicts-pane.svelte';

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
  // KEY FIX: Use pane.id instead of global navigationStore.state.activePaneId
  const tabs = $derived(navigationStore.state.tabs);
  const activeTabId = $derived(navigationStore.state.activeTabIds[pane.id]); // ✅ Use THIS pane's ID
  const activeTab = $derived(tabs.find((t) => t.id === activeTabId));

  // Track loaded viewer components by nodeType
  let viewerComponents = $state<Map<string, unknown>>(new Map());
  let viewerLoadErrors = $state<Map<string, string>>(new Map());
  let viewerLoading = $state<Set<string>>(new Set());

  // Load viewer when needed - moved to function called from onMount to avoid derived context issues
  async function loadViewerForNodeType(nodeType: string) {
    if (
      viewerComponents.has(nodeType) ||
      viewerLoadErrors.has(nodeType) ||
      viewerLoading.has(nodeType)
    ) {
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
      // repeated load attempts that cause mount/unmount loops.
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

  // Ensure the node backing a tab is present in the store AND not possibly stale from a
  // daemon reconnect (a WatchNodes outage can silently drop the update that would have kept
  // an already-cached node current — see SharedNodeStore.isPossiblyStale). This only pushes a
  // fetch into sharedNodeStore (a non-reactive external system) — it writes no local reactive
  // state, so there is nothing to race: hydration status is read back off the store's
  // per-node cell via isNodeHydrated below. If the node genuinely doesn't exist, close the
  // tab — but only if that tab still points at this nodeId, so a superseded navigation can't
  // close the newly-active tab (checked against live tab state, the single source of truth,
  // not a tracked "latest requested" variable).
  async function hydrateNode(nodeId: string, tabId: string) {
    if (sharedNodeStore.getNode(nodeId) && !sharedNodeStore.isPossiblyStale(nodeId)) return;

    let node;
    try {
      node = await sharedNodeStore.ensureNode(nodeId);
    } catch (error) {
      log.error(`Failed to hydrate node ${nodeId}:`, error);
      return;
    }

    if (!node) {
      const tab = untrack(() => navigationStore.state.tabs.find((t) => t.id === tabId));
      if (tab?.content?.nodeId === nodeId) {
        log.warn(`Node ${nodeId} not found — closing stale tab`);
        closeTab(tabId);
      }
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

  const isNodeHydrated = $derived.by(() => {
    const nodeId = activeTab?.content?.nodeId;
    if (!nodeId) return true; // settings tabs and placeholder tabs need no hydration
    // Read the store's per-node cell directly: a node is hydrated once it is present.
    return sharedNodeStore.getNode(nodeId) != null;
  });

  // When the active tab changes, push the two side effects it requires into their
  // subsystems: lazy-load the viewer module for the node type, and ensure the node is
  // present in sharedNodeStore. The effect body writes no reactive state of its own
  // (ADR-049) — viewer results land in the module cache, hydration lands in the store,
  // and both are read back via the $derived values above. tabId is captured here so a
  // not-found close targets the tab that triggered the fetch.
  $effect(() => {
    const nodeType = activeTab?.content?.nodeType;
    const nodeId = activeTab?.content?.nodeId;
    const tabId = activeTabId;
    if (nodeType) {
      untrack(() => loadViewerForNodeType(nodeType));
    }
    if (nodeId && tabId) {
      untrack(() => hydrateNode(nodeId, tabId));
    }
  });

  // A hydration fetch that fails is otherwise permanent: the $effect above only
  // re-runs on tab changes, so a pane whose first fetch raced the daemon's boot
  // window (fresh install: the webview renders while the sidecar is still
  // seeding its DB and binding the socket) sits on "Loading..." forever. Retry
  // the active tab's hydration whenever the daemon transitions back to healthy.
  // This also covers the case where the active tab's node WAS already hydrated
  // before the outage: hydrateNode's own presence-AND-freshness guard decides
  // whether a re-fetch is actually needed, so this handler can unconditionally
  // delegate to it rather than duplicating that check; same idiom as the
  // sidebar stores and base-node-viewer.
  onMount(() =>
    onDaemonReconnect(() => {
      const nodeId = activeTab?.content?.nodeId;
      const tabId = activeTabId;
      if (nodeId && tabId) {
        hydrateNode(nodeId, tabId);
      }
    })
  );
</script>

{#if activeTab?.type === 'settings'}
  <SettingsPane />
{:else if activeTab?.type === 'search'}
  <SearchPane />
{:else if activeTab?.type === 'conflicts'}
  <ConflictsPane />
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
  {:else if isViewerLoading || !isNodeHydrated}
    <!-- Viewer module still loading or parent node not yet hydrated -->
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
