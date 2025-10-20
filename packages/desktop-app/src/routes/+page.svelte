<script lang="ts">
  import DateNodeViewer from '$lib/components/viewers/date-node-viewer.svelte';
  import TextNodeViewer from '$lib/components/viewers/text-node-viewer.svelte';
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
  {#if activeTab.type === 'date' || activeTabId === 'today'}
    <DateNodeViewer tabId={activeTabId} />
  {:else if activeTab.type === 'node' && activeTab.content && typeof activeTab.content === 'object' && 'nodeType' in activeTab.content && 'nodeId' in activeTab.content}
    <!-- Dynamic node viewer based on nodeType -->
    {#if activeTab.content.nodeType === 'text'}
      <TextNodeViewer nodeId={String(activeTab.content.nodeId)} />
    {:else}
      <!-- For now, show placeholder for other node types -->
      <div class="placeholder-content">
        <h2>{activeTab.title}</h2>
        <p>Node type "{activeTab.content.nodeType}" viewer coming soon...</p>
        <p class="node-id">Node ID: {activeTab.content.nodeId}</p>
      </div>
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

  .placeholder-content .node-id {
    font-family: monospace;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground) / 0.7);
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
