<script lang="ts">
  import DateNodeViewer from '$lib/components/viewers/date-node-viewer.svelte';
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';
  import { toggleTheme } from '$lib/design/theme.js';
  import { tabState, updateTabTitle } from '$lib/stores/navigation.js';

  // Derive tab state using Svelte 5 $derived
  const tabs = $derived($tabState.tabs);
  const activeTabId = $derived($tabState.activeTabId);
  const activeTab = $derived(tabs.find((t) => t.id === activeTabId));

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
  <!-- Route based on nodeType -->
  {@const content = activeTab.content}

  {#if content.nodeType === 'date'}
    <!-- DateNodeViewer: Page-level viewer with date navigation UI -->
    <DateNodeViewer
      nodeId={content.nodeId}
      onTitleChange={(title) => updateTabTitle(activeTabId, title)}
    />
  {:else}
    <!-- BaseNodeViewer: Default fallback for all other node types -->
    <!-- Shows the node in context with its children -->
    <BaseNodeViewer
      nodeId={content.nodeId}
      onTitleChange={(title) => updateTabTitle(activeTabId, title)}
    />
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
</style>
