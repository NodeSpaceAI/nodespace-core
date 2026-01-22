<!--
  PaneManager Component

  Manages multiple side-by-side panes, each with its own tab system.

  Features:
  - Maximum 2 panes supported (side-by-side vertical split)
  - Each pane has its own TabSystem with separate tab bar
  - Draggable resize handle between panes
  - Active pane tracking (clicking in pane makes it active)
  - Auto-close empty panes when last tab closed

  Resize Handle:
  - 4px wide hit area for dragging
  - Minimum 200px per pane enforced
  - Smooth drag experience with cursor feedback
  - Maintains 100% total width

  Integration:
  - Uses navigation store (tabState, setActivePane, resizePane)
  - Slot pattern for content with activeTab prop per pane
  - Passes through tab events to store

  Usage:
  <PaneManager>
    Snippet: children({ activeTab })
      Content based on activeTab
  </PaneManager>
-->
<script lang="ts">
  import {
    tabState,
    setActivePane,
    resizePane,
    getOrderedTabsForPane,
    type Tab
  } from '$lib/stores/navigation.js';
  import TabSystem from './tab-system.svelte';
  import PaneContent from './pane-content.svelte';
  import { cn } from '$lib/utils.js';
  import { throttle } from '$lib/utils/throttle.js';

  // Props - no longer need children snippet, we render PaneContent directly

  // Resize state
  let resizing = $state(false);
  let startX = $state(0);
  let startWidth = $state(0);
  let containerWidth = $state(0);
  let containerElement: HTMLElement | null = $state(null);

  // Start resize operation
  function handleResizeStart(event: MouseEvent): void {
    if (!containerElement) return;

    resizing = true;
    startX = event.clientX;
    containerWidth = containerElement.clientWidth;

    // Get first pane's current width in pixels
    const firstPane = $tabState.panes[0];
    if (firstPane) {
      startWidth = (firstPane.width / 100) * containerWidth;
    }

    // Prevent text selection during drag
    event.preventDefault();
  }

  // Handle resize drag movement (throttled for performance)
  const handleResizeMove = throttle(function (event: MouseEvent): void {
    if (!resizing || !containerElement) return;

    const delta = event.clientX - startX;
    const newWidth = startWidth + delta;

    // Calculate minimum width in pixels (200px)
    const minWidth = 200;
    const maxWidth = containerWidth - minWidth;

    // Enforce minimum widths
    if (newWidth < minWidth || newWidth > maxWidth) {
      return;
    }

    // Convert to percentage and update first pane
    const newWidthPercent = (newWidth / containerWidth) * 100;
    const firstPaneId = $tabState.panes[0].id;
    resizePane(firstPaneId, newWidthPercent);
  }, 16); // ~60fps throttling for smooth resize

  // End resize operation
  function handleResizeEnd(): void {
    resizing = false;
  }

  // Get tabs for a specific pane in the correct order based on pane.tabIds
  // Uses the shared utility function for consistent tab ordering
  function getTabsForPane(paneId: string): Tab[] {
    return getOrderedTabsForPane($tabState, paneId);
  }

  // Get active tab ID for a specific pane
  function getActiveTabId(paneId: string): string {
    return $tabState.activeTabIds[paneId] || '';
  }

  // Handle keyboard resize for accessibility (WCAG 2.1 Level AA)
  function handleResizeKeyboard(event: KeyboardEvent): void {
    const firstPane = $tabState.panes[0];
    if (!firstPane || $tabState.panes.length < 2) return;

    const step = event.shiftKey ? 10 : 5; // Larger steps with Shift key

    if (event.key === 'ArrowLeft') {
      event.preventDefault();
      const newWidth = Math.max(20, Math.min(80, firstPane.width - step));
      resizePane(firstPane.id, newWidth);
    } else if (event.key === 'ArrowRight') {
      event.preventDefault();
      const newWidth = Math.max(20, Math.min(80, firstPane.width + step));
      resizePane(firstPane.id, newWidth);
    }
  }
</script>

<!-- Window-level mouse event handlers for resize -->
<svelte:window on:mousemove={handleResizeMove} on:mouseup={handleResizeEnd} />

<div class="pane-manager" bind:this={containerElement}>
  {#each $tabState.panes as pane, index (pane.id)}
    {@const activeTabId = getActiveTabId(pane.id)}

    <!-- Pane wrapper -->
    <div
      class={cn('pane', pane.id === $tabState.activePaneId && 'pane--active')}
      style="width: {pane.width}%"
      data-pane-id={pane.id}
      onclick={() => setActivePane(pane.id)}
      onkeydown={(e) => {
        // Only handle keyboard activation when the pane container itself is focused
        // Don't intercept keys when child elements (textareas, inputs) are focused
        if ((e.key === 'Enter' || e.key === ' ') && e.target === e.currentTarget) {
          e.preventDefault();
          setActivePane(pane.id);
        }
      }}
      role="button"
      aria-label="Activate Pane {index + 1}"
      tabindex="0"
    >
      <!-- Each pane gets its own TabSystem -->
      <TabSystem tabs={getTabsForPane(pane.id)} {activeTabId} {pane}>
        {#snippet children({ activeTab: _tabForPane })}
          <!-- Render PaneContent directly - it receives the pane and derives its own activeTab -->
          <!-- Note: _tabForPane is unused here because PaneContent derives activeTab from pane.id -->
          <PaneContent {pane} />
        {/snippet}
      </TabSystem>
    </div>

    <!-- Resize handle between panes -->
    {#if index < $tabState.panes.length - 1}
      <button
        class={cn('resize-handle', resizing && 'resize-handle--active')}
        aria-label="Resize panes - drag or use arrow keys to adjust width"
        onmousedown={handleResizeStart}
        onkeydown={handleResizeKeyboard}
        type="button"
      ></button>
    {/if}
  {/each}
</div>

<style>
  .pane-manager {
    display: flex;
    width: 100%;
    height: 100%;
    position: relative;
    overflow: hidden;
  }

  .pane {
    display: flex;
    flex-direction: column;
    min-height: 0;
    position: relative;
    background-color: hsl(var(--content-background));
    /* Subtle border to indicate pane boundary */
    border-right: 1px solid hsl(var(--border));
    /* Override browser default cursor for role="button" - pane is clickable but not a typical button */
    cursor: default;
  }

  /* Remove border from last pane */
  .pane:last-child {
    border-right: none;
  }

  /* Active pane visual indicator */
  .pane--active {
    /* Subtle highlight to show which pane is active */
    box-shadow: inset 0 0 0 1px hsl(var(--ring) / 0.2);
  }

  /* Resize handle */
  .resize-handle {
    width: 4px;
    background-color: hsl(var(--border));
    cursor: col-resize;
    position: relative;
    flex-shrink: 0;
    transition: background-color 0.15s ease;
    /* Ensure handle is above pane content */
    z-index: 10;
    /* Button reset */
    border: none;
    padding: 0;
    height: 100%;
    outline: none;
  }

  .resize-handle:hover,
  .resize-handle--active {
    background-color: hsl(var(--ring));
  }

  /* Increase hit area for easier grabbing */
  .resize-handle::before {
    content: '';
    position: absolute;
    top: 0;
    left: -4px;
    right: -4px;
    bottom: 0;
    /* Invisible but extends hit area */
  }

  /* Prevent text selection during resize */
  .pane-manager:has(.resize-handle--active) {
    user-select: none;
  }

  /* Apply col-resize cursor to body when resizing */
  .pane-manager:has(.resize-handle--active) * {
    cursor: col-resize !important;
  }
</style>
