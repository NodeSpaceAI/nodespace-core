<script lang="ts">
  import DateNodeViewer from '$lib/components/viewers/date-node-viewer.svelte';
  import { toggleTheme } from '$lib/design/theme.js';
  import { tabState } from '$lib/stores/navigation.js';

  // Subscribe to tab state from store
  $: ({ tabs, activeTabId } = $tabState);

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

{#if activeTabId === 'today'}
  <DateNodeViewer tabId="today" />
{:else}
  <!-- Placeholder content for other tabs -->
  <div class="placeholder-content">
    <h2>{tabs.find(t => t.id === activeTabId)?.title}</h2>
    <p>This is a placeholder tab. Content will be implemented later.</p>
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
    margin: 0;
    color: hsl(var(--muted-foreground));
  }
</style>