<script lang="ts">
  import DateNodeViewer from '$lib/components/viewers/date-node-viewer.svelte';
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';
  import { toggleTheme } from '$lib/design/theme.js';
  import { tabState } from '$lib/stores/navigation.js';

  // Subscribe to tab state from store
  $: ({ tabs, activeTabId } = $tabState);
  $: activeTab = tabs.find((t) => t.id === activeTabId);

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

{#if activeTab}
  {#if activeTab.type === 'node' && activeTab.content}
    <!-- Route based on nodeType - each viewer is responsible for its own tab title -->
    {@const content = activeTab.content}
    {#if content.nodeType === 'date'}
      <!-- DateNodeViewer sets tab title to "Today", "Tomorrow", or formatted date -->
      <DateNodeViewer tabId={activeTabId} initialDate={content.nodeId} />
    {:else}
      <!-- BaseNodeViewer sets tab title to node content (first line) -->
      <BaseNodeViewer parentId={content.nodeId} />
    {/if}
  {:else}
    <!-- Placeholder content for other tab types -->
    <div class="placeholder-content">
      <h2>{activeTab.title}</h2>
      <p>This is a placeholder tab. Content will be implemented later.</p>
    </div>
  {/if}
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
