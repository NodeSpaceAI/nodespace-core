<!--
  DateNodeViewer - Page-level viewer that wraps BaseNodeViewer with date navigation

  Features:
  - Date navigation header with professional styling
  - Previous/next day functionality
  - Keyboard support for arrow key navigation
  - Database integration for persistent daily journal entries
  - Lazy date node creation when first child is added
  - Debounced saving (500ms) for user input
  - Seamless integration with existing BaseNodeViewer
  - Clean date formatting (e.g., "September 7, 2025")

  Follows the *NodeViewer pattern (like TaskNodeViewer) for page-level viewers.
-->

<script lang="ts">
  import BaseNodeViewer from '$lib/design/components/base-node-viewer.svelte';
  import Icon from '$lib/design/icons/icon.svelte';
  import { getDateTabTitle } from '$lib/stores/navigation.js';

  // Props using Svelte 5 runes mode - unified NodeViewerProps interface
  let { nodeId, onTitleChange }: { nodeId: string; onTitleChange?: (_title: string) => void } =
    $props();

  // Normalize date to midnight local time (ignore time components)
  function normalizeDate(d: Date): Date {
    return new Date(d.getFullYear(), d.getMonth(), d.getDate());
  }

  // Parse date from nodeId prop (format: YYYY-MM-DD)
  function parseDateFromNodeId(dateString: string): Date {
    // Parse YYYY-MM-DD format
    const [year, month, day] = dateString.split('-').map(Number);
    if (year && month && day) {
      return normalizeDate(new Date(year, month - 1, day)); // month is 0-indexed
    }
    return normalizeDate(new Date()); // Fallback to today if invalid
  }

  // Current date state - initialized from nodeId prop
  let currentDate = $state(parseDateFromNodeId(nodeId));

  // Sync currentDate when nodeId prop changes (e.g., tab switching)
  $effect(() => {
    currentDate = parseDateFromNodeId(nodeId);
  });

  // Format date for display (e.g., "September 7, 2025") using Svelte 5 $derived
  const formattedDate = $derived(
    currentDate.toLocaleDateString('en-US', {
      year: 'numeric',
      month: 'long',
      day: 'numeric'
    })
  );

  // Derive current date ID from currentDate (using local timezone, not UTC)
  const currentDateId = $derived(
    `${currentDate.getFullYear()}-${String(currentDate.getMonth() + 1).padStart(2, '0')}-${String(currentDate.getDate()).padStart(2, '0')}`
  );

  // Update tab title when date changes using Svelte 5 $effect
  $effect(() => {
    const newTitle = getDateTabTitle(currentDate);
    onTitleChange?.(newTitle);
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
    currentDate = normalizeDate(newDate);
  }

  /**
   * Handle keyboard navigation for date switching
   */
  function handleKeydown(event: KeyboardEvent) {
    // Only handle if focus is not on editable elements (contenteditable or textarea/input)
    const activeElement = document.activeElement as HTMLElement;
    const isContentEditable = activeElement && activeElement.contentEditable === 'true';
    const isTextInput =
      activeElement && (activeElement.tagName === 'TEXTAREA' || activeElement.tagName === 'INPUT');
    const isTextEditor = isContentEditable || isTextInput;

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

<BaseNodeViewer nodeId={currentDateId} {onTitleChange}>
  {#snippet header()}
    <!-- Date Navigation Header - inherits base styling from BaseNodeViewer -->
    <div class="date-nav-container">
      <div class="date-display">
        <Icon
          name="calendar"
          size={24}
          color="hsl(var(--muted-foreground))"
          className="calendar-icon"
        />
        <h1>{formattedDate}</h1>
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
  {/snippet}
</BaseNodeViewer>

<style>
  /* Date-specific navigation styles - base header styling comes from BaseNodeViewer */
  .date-nav-container {
    display: flex;
    align-items: center;
    justify-content: space-between;
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

  /* Calendar icon styling */
  :global(.calendar-icon) {
    opacity: 0.8;
  }

  /* Left arrow rotation for previous button */
  :global(.rotate-left) {
    transform: rotate(180deg);
  }
</style>
