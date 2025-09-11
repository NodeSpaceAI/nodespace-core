<!--
  DatePageViewer - Specialized viewer that wraps BaseNodeViewer with date navigation
  
  Features:
  - Date navigation header with professional styling
  - Previous/next day functionality
  - Keyboard support for arrow key navigation
  - Seamless integration with existing BaseNodeViewer
  - Clean date formatting (e.g., "September 7, 2025")
-->

<script lang="ts">
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';
  import Icon from '$lib/design/icons/icon.svelte';
  import { updateTabTitle, getDateTabTitle } from '$lib/stores/navigation.js';

  // Props using Svelte 5 runes mode
  let { tabId = 'today' }: { tabId?: string } = $props();

  // Current date state - defaults to today
  let currentDate = $state(new Date());

  // Format date for display (e.g., "September 7, 2025") using Svelte 5 $derived
  const formattedDate = $derived(
    currentDate.toLocaleDateString('en-US', {
      year: 'numeric',
      month: 'long',
      day: 'numeric'
    })
  );

  // Update tab title when date changes using Svelte 5 $effect
  $effect(() => {
    if (tabId) {
      const newTitle = getDateTabTitle(currentDate);
      updateTabTitle(tabId, newTitle);
    }
  });

  /**
   * Navigate to previous or next day
   */
  function navigateDate(direction: 'prev' | 'next') {
    const newDate = new Date(currentDate);
    if (direction === 'prev') {
      newDate.setDate(newDate.getDate() - 1);
    } else {
      newDate.setDate(newDate.getDate() + 1);
    }
    currentDate = newDate;
  }

  /**
   * Handle keyboard navigation for date switching
   */
  function handleKeydown(event: KeyboardEvent) {
    // Only handle if focus is not on contenteditable elements
    const activeElement = document.activeElement as HTMLElement;
    const isTextEditor = activeElement && activeElement.contentEditable === 'true';

    if (isTextEditor) return;

    if (event.key === 'ArrowLeft') {
      event.preventDefault();
      navigateDate('prev');
    } else if (event.key === 'ArrowRight') {
      event.preventDefault();
      navigateDate('next');
    }
  }
</script>

<!-- Keyboard event listener -->
<svelte:window on:keydown={handleKeydown} />

<div class="date-page-viewer">
  <!-- Date Navigation Header - matches original +page.svelte design -->
  <div class="date-header">
    <div class="date-nav-container">
      <div class="date-display">
        <Icon
          name="calendar"
          size={18}
          color="hsl(var(--muted-foreground))"
          className="calendar-icon"
        />
        <h1
          style="font-size: 2rem; font-weight: 500; color: hsl(var(--muted-foreground)); margin: 0;"
        >
          {formattedDate}
        </h1>
      </div>

      <div class="date-nav-buttons">
        <button class="date-nav-btn" onclick={() => navigateDate('prev')} aria-label="Previous day">
          <Icon name="chevronRight" size={16} color="currentColor" className="rotate-left" />
        </button>
        <button class="date-nav-btn" onclick={() => navigateDate('next')} aria-label="Next day">
          <Icon name="chevronRight" size={16} color="currentColor" />
        </button>
      </div>
    </div>
  </div>

  <!-- Node Content Area -->
  <div class="node-content-area">
    <BaseNodeViewer />
  </div>
</div>

<style>
  .date-page-viewer {
    /* Full height container for the entire date viewer */
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
  }

  /* Date header styling - matches original +page.svelte design exactly */
  .date-header {
    border-bottom: 1px solid hsl(var(--border));
    background: hsl(var(--background));
    flex-shrink: 0;
  }

  .date-nav-container {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1rem;
  }

  .date-display {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .date-nav-buttons {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .date-nav-btn {
    width: 32px;
    height: 32px;
    border: 1px solid hsl(var(--border));
    border-radius: 0.375rem;
    background: hsl(var(--background));
    color: hsl(var(--muted-foreground));
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .date-nav-btn:hover {
    background: hsl(var(--muted));
    color: hsl(var(--foreground));
  }

  .node-content-area {
    /* Container for the BaseNodeViewer - takes remaining space */
    flex: 1;
    overflow: auto;
    display: flex;
    flex-direction: column;
    /* Add proper padding for node tree spacing - matches original editor-area */
    padding: 1.5rem;
  }

  /* Calendar icon styling */
  :global(.calendar-icon) {
    opacity: 0.8;
  }

  /* Left arrow rotation for previous button */
  :global(.rotate-left) {
    transform: rotate(180deg);
  }
</style>
