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
  import { onMount } from 'svelte';
  import { parseDateString, formatDateISO, normalizeDate, formatDateTitle } from '$lib/utils/date-formatting';

  // Props using Svelte 5 runes mode - unified NodeViewerProps interface
  // onTitleChange called from event handlers (not effects) to update tab title
  let {
    nodeId,
    onNodeIdChange,
    onTitleChange
  }: {
    nodeId: string;
    onNodeIdChange?: (_nodeId: string) => void;
    onTitleChange?: (_title: string) => void;
  } = $props();

  // Set initial tab title on mount - viewer is the source of truth for tab titles
  onMount(() => {
    const date = parseDateFromNodeId(nodeId);
    onTitleChange?.(formatDateTitle(date));
  });

  // Parse date from nodeId prop (format: YYYY-MM-DD)
  function parseDateFromNodeId(dateString: string): Date {
    const parsed = parseDateString(dateString);
    return parsed ?? normalizeDate(new Date()); // Fallback to today if invalid
  }

  // Derive current date from nodeId prop (single source of truth)
  // Frontend-architect recommendation: Use $derived instead of $state
  const currentDate = $derived(parseDateFromNodeId(nodeId));

  // Format date for display (e.g., "September 7, 2025")
  const formattedDate = $derived(
    currentDate.toLocaleDateString('en-US', {
      year: 'numeric',
      month: 'long',
      day: 'numeric'
    })
  );

  // Derive current date ID from currentDate (using local timezone, not UTC)
  const currentDateId = $derived(formatDateISO(currentDate));

  /**
   * Navigate to previous or next day
   * Calls onNodeIdChange to update content, and onTitleChange to update tab title
   * Both are event-driven (not effect-driven) - no $effect anti-pattern
   */
  function navigateDate(direction: 'prev' | 'next') {
    const newDate = new Date(currentDate);
    if (direction === 'prev') {
      newDate.setDate(newDate.getDate() - 1);
    } else {
      newDate.setDate(newDate.getDate() + 1);
    }
    const normalizedDate = normalizeDate(newDate);
    const newNodeId = formatDateISO(normalizedDate);

    // Update content (nodeId) and tab title via parent callbacks
    // This is event-driven, not effect-driven - proper pattern
    onNodeIdChange?.(newNodeId);
    onTitleChange?.(formatDateTitle(normalizedDate));
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

<div class="date-node-viewer">
  <BaseNodeViewer nodeId={currentDateId} disableTitleUpdates={true}>
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
          <button
            class="date-nav-btn"
            onclick={() => navigateDate('prev')}
            aria-label="Previous day"
          >
            <Icon name="chevronRight" size={16} color="currentColor" className="rotate-left" />
          </button>
          <button class="date-nav-btn" onclick={() => navigateDate('next')} aria-label="Next day">
            <Icon name="chevronRight" size={16} color="currentColor" />
          </button>
        </div>
      </div>
    {/snippet}
  </BaseNodeViewer>
</div>

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
