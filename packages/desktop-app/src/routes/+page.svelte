<script lang="ts">
  import DateNodeViewer from '$lib/components/viewers/date-node-viewer.svelte';
  import { toggleTheme } from '$lib/design/theme.js';
  import { tabState, setActiveTab, closeTab } from '$lib/stores/navigation.js';
  import { writable } from 'svelte/store';

  // Subscribe to tab state from store
  $: ({ tabs, activeTabId } = $tabState);

  function handleTabClick(tabId: string) {
    setActiveTab(tabId);
  }

  function handleTabClose(tabId: string) {
    closeTab(tabId);
  }


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

    <!-- Main content area - no container wrapper needed, AppShell handles layout -->
    <main class="main-content" style="padding: 0; height: 100vh; overflow: hidden;">
      <!-- Tab system -->
      {#if tabs.length > 1}
        <div class="tab-bar">
          {#each tabs as tab}
            <button 
              class="tab-item" 
              class:active={activeTabId === tab.id}
              on:click={() => handleTabClick(tab.id)}
            >
              <span class="tab-title">
                {tab.title.length > 25 ? tab.title.substring(0, 25) + '...' : tab.title}
              </span>
              {#if tab.closeable}
                <button class="tab-close" on:click|stopPropagation={() => handleTabClose(tab.id)}>×</button>
              {/if}
            </button>
          {/each}
        </div>
      {/if}

      <!-- Tab content -->
      <div class="tab-content">
        {#if activeTabId === 'today'}
          <DateNodeViewer tabId="today" />
        {:else}
          <!-- Placeholder content for other tabs -->
          <div class="placeholder-content">
            <h2>{tabs.find(t => t.id === activeTabId)?.title}</h2>
            <p>This is a placeholder tab. Content will be implemented later.</p>
          </div>
        {/if}
      </div>
    </main>

<style>
  /* App container with grid layout */
  .app-container {
    height: 100vh;
    display: grid;
    grid-template-columns: auto 1fr;
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
  }

  /* Main content */
  .main-content {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  /* Tab bar - Browser-Like Tab System from patterns.html */
  .tab-bar {
    height: 40px;
    background: hsl(var(--muted));
    border-bottom: 1px solid hsl(var(--border));
    display: flex;
    align-items: stretch;
    padding: 0;
  }

  .tab-item {
    padding: 0 1rem;
    background: hsl(var(--muted));
    border: none;
    border-right: 1px solid hsl(var(--border));
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
    cursor: pointer;
    font-weight: 500;
    transition: background-color 0.2s, color 0.2s;
  }

  .tab-item:hover {
    background: hsl(var(--hover-background));
    color: hsl(var(--hover-foreground));
  }

  .tab-item.active {
    background: hsl(var(--background));
    color: hsl(var(--foreground));
    font-weight: 600;
    position: relative;
    z-index: 2;
    margin-bottom: -1px;
  }

  /* Active tab accent on top - extends over tab borders */
  .tab-item.active::before {
    content: '';
    position: absolute;
    top: 0;
    left: -1px;
    right: -1px;
    height: 4px;
    background: hsl(var(--primary));
    z-index: 3;
  }

  .tab-close {
    background: none;
    border: none;
    color: hsl(var(--muted-foreground));
    cursor: pointer;
    font-size: 1.2rem;
    padding: 0;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .tab-close:hover {
    background: hsl(var(--destructive));
    color: hsl(var(--destructive-foreground));
  }

  /* Tab content */
  .tab-content {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

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

  /* Icon System CSS from patterns.html */
  .task-icon {
    width: var(--circle-diameter, 20px);
    height: var(--circle-diameter, 20px);
    position: relative;
    display: block;
  }

  .node-icon {
    width: var(--circle-diameter, 20px);
    height: var(--circle-diameter, 20px);
    position: relative;
    display: block;
  }

  .text-circle {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: hsl(var(--primary));
    position: absolute;
    top: 2px;
    left: 2px;
  }

  .task-circle {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    position: absolute;
    top: 2px;
    left: 2px;
  }

  .task-circle-pending {
    background: transparent;
    border: 1px solid hsl(var(--primary));
    box-sizing: border-box;
  }

  .task-circle-in-progress {
    background: linear-gradient(90deg, hsl(var(--primary)) 50%, transparent 50%);
    border: 1px solid hsl(var(--primary));
    box-sizing: border-box;
  }

  .task-circle-completed {
    background: hsl(var(--primary));
  }

  /* Checkmark for completed tasks - theme-aware stroke color */
  .task-circle-completed::after {
    content: "";
    position: absolute;
    left: 50%;
    top: 50%;
    width: 10px;
    height: 8px;
    transform: translate(-50%, -50%);
    background-image: url("data:image/svg+xml,%3Csvg width='10' height='8' viewBox='0 0 10 8' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 4L3.5 6.5L9 1' stroke='%23FAF9F5' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: center;
    background-size: contain;
  }

  .dark .task-circle-completed::after {
    background-image: url("data:image/svg+xml,%3Csvg width='10' height='8' viewBox='0 0 10 8' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 4L3.5 6.5L9 1' stroke='%23252523' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: center;
    background-size: contain;
  }

  .task-ring {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    border: 2px solid hsl(var(--primary) / 0.5);
    box-sizing: border-box;
    position: absolute;
    top: 0;
    left: 0;
  }
</style>