<!-- 
  TabSystem Component
  
  A browser-like tab system that manages multiple content tabs according to the NodeSpace design system.
  
  Features:
  - Single tab: No tab bar shown, just content
  - Multiple tabs: Show tab bar with truncated titles (~25 characters)
  - Active tab: White background, full opacity text
  - Inactive tabs: Muted background, reduced opacity
  - Tab closing: Close button for closeable tabs
  - Tab switching: Click to activate tabs
  - Keyboard navigation: Arrow keys, Enter/Space
  - Accessibility: ARIA labels, proper focus management

  Close Button Design:
  - Size: 12×12px button with 8×8px CSS-only X icon (compact, subtle appearance)
  - Position: 2px from top-right corner of each tab (close to corner)
  - Interaction: Hidden by default, fades in on tab hover (opacity: 0.6), full opacity on direct hover
  - Line weight: 1px for crisp, delicate appearance
  - Keyboard accessible: Tab key navigation with focus indicators
  
  Integration:
  - Uses navigation store (tabState, setActiveTab, closeTab)
  - Slot pattern for content with activeTab prop
  - Design system compliant colors and styling
  
  Usage:
  <TabSystem>
    Snippet: children({ activeTab })
      Content based on activeTab
  </TabSystem>
-->
<script lang="ts">
  import {
    tabState,
    setActiveTab,
    closeTab,
    reorderTab,
    moveTabBetweenPanes,
    getOrderedTabsForPane
  } from '$lib/stores/navigation.js';
  import { cn } from '$lib/utils.js';
  import type { Tab, Pane } from '$lib/stores/navigation.js';
  import type { Snippet } from 'svelte';
  import { draggable, droppable, type DragDropState } from '@thisux/sveltednd';

  // Props - when used inside PaneManager
  let {
    tabs = undefined,
    activeTabId = undefined,
    pane = undefined,
    children
  }: {
    tabs?: Tab[];
    activeTabId?: string;
    pane?: Pane;
    children?: Snippet<[{ activeTab: Tab | undefined }]>;
  } = $props();

  // Fallback to global state when not pane-specific (backwards compatibility)
  const currentPaneId = $derived(pane?.id || $tabState.activePaneId);

  // Get tabs in the correct order based on pane's tabIds array
  // Use the optimized utility function for consistent tab ordering
  const displayTabs = $derived(tabs || getOrderedTabsForPane($tabState, currentPaneId));

  const currentActiveTabId = $derived(
    activeTabId || $tabState.activeTabIds[$tabState.activePaneId]
  );

  // Check if close button should be disabled (last tab in last pane)
  const isCloseDisabled = $derived(displayTabs.length === 1 && $tabState.panes.length === 1);

  // Drag-over state for visual feedback
  let dragOverIndex: number | null = $state(null);
  let draggingTabId: string | null = $state(null);

  // Truncate title to specified length with ellipsis
  function truncateTitle(title: string, maxLength: number = 25): string {
    return title.length > maxLength ? title.substring(0, maxLength - 1) + '…' : title;
  }

  // Handle tab click to switch active tab
  function handleTabClick(tabId: string): void {
    setActiveTab(tabId, currentPaneId);
  }

  // Handle keyboard navigation for accessibility
  function handleTabKeydown(event: KeyboardEvent, tabId: string): void {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      setActiveTab(tabId, currentPaneId);
    } else if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
      event.preventDefault();
      const currentIndex = displayTabs.findIndex((tab) => tab.id === tabId);

      let nextIndex: number;
      if (event.key === 'ArrowRight') {
        nextIndex = currentIndex === displayTabs.length - 1 ? 0 : currentIndex + 1;
      } else {
        nextIndex = currentIndex === 0 ? displayTabs.length - 1 : currentIndex - 1;
      }

      setActiveTab(displayTabs[nextIndex].id, currentPaneId);
    }
  }

  // Handle close button click
  function handleCloseTab(event: MouseEvent, tabId: string): void {
    event.stopPropagation(); // Prevent tab activation

    // Cannot close last tab in last pane
    if (isCloseDisabled) {
      return;
    }

    const tab = displayTabs.find((t) => t.id === tabId);
    if (tab && tab.closeable) {
      closeTab(tabId);
    }
  }

  // Drag-and-drop handlers using @thisux/sveltednd

  /**
   * Handle drag start - track which tab is being dragged
   */
  function handleDragStart(state: DragDropState<{ tab: Tab; paneId: string }>): void {
    if (state.draggedItem) {
      draggingTabId = state.draggedItem.tab.id;
    }
  }

  /**
   * Handle drag end - clear dragging state
   */
  function handleDragEnd(): void {
    draggingTabId = null;
    dragOverIndex = null;
  }

  /**
   * Handle drag over - show visual indicator
   */
  function handleDragOver(state: DragDropState<{ tab: Tab; paneId: string }>): void {
    const { targetContainer } = state;
    if (targetContainer) {
      const targetIndex = parseInt(targetContainer.replace('tab-', ''));
      if (!isNaN(targetIndex)) {
        dragOverIndex = targetIndex;
      }
    }
  }

  /**
   * Handle drag enter - show visual indicator
   */
  function handleDragEnter(state: DragDropState<{ tab: Tab; paneId: string }>): void {
    handleDragOver(state);
  }

  /**
   * Handle drag leave - clear visual indicator
   */
  function handleDragLeave(): void {
    dragOverIndex = null;
  }

  /**
   * Handle drop - perform tab reorder or move between panes
   */
  function handleDrop(state: DragDropState<{ tab: Tab; paneId: string }>): void {
    try {
      const { draggedItem, targetContainer, sourceContainer } = state;

      // Early validation: check for required state
      if (!draggedItem || !targetContainer || !sourceContainer) {
        console.warn('[TabSystem] Invalid drop operation: missing required state', {
          hasDraggedItem: !!draggedItem,
          hasTargetContainer: !!targetContainer,
          hasSourceContainer: !!sourceContainer
        });
        dragOverIndex = null;
        return;
      }

      // Parse and validate indices
      const sourceIndex = parseInt(sourceContainer.replace('tab-', ''));
      const targetIndex = parseInt(targetContainer.replace('tab-', ''));

      if (isNaN(sourceIndex) || isNaN(targetIndex)) {
        console.error('[TabSystem] Invalid drop operation: could not parse container indices', {
          sourceContainer,
          targetContainer,
          sourceIndex,
          targetIndex,
          draggedTab: draggedItem.tab.title,
          currentPaneId,
          totalTabs: displayTabs.length
        });
        dragOverIndex = null;
        return;
      }

      const sourcePaneId = draggedItem.paneId;
      const tabId = draggedItem.tab.id;

      // Validate pane existence
      const sourcePaneExists = $tabState.panes.some((p) => p.id === sourcePaneId);
      const targetPaneExists = $tabState.panes.some((p) => p.id === currentPaneId);

      if (!sourcePaneExists || !targetPaneExists) {
        console.error('[TabSystem] Invalid drop operation: pane not found', {
          sourcePaneId,
          targetPaneId: currentPaneId,
          sourcePaneExists,
          targetPaneExists,
          draggedTab: draggedItem.tab.title,
          sourceIndex,
          targetIndex,
          totalPanes: $tabState.panes.length
        });
        dragOverIndex = null;
        return;
      }

      // Skip if dropping on same position
      if (sourcePaneId === currentPaneId && sourceIndex === targetIndex) {
        dragOverIndex = null;
        return;
      }

      // Perform the drop operation
      if (sourcePaneId === currentPaneId) {
        // Within-pane reorder
        // When dragging forward (left to right), we need to adjust the target index
        // because removing the source shifts all subsequent indices down by 1
        // The visual drop indicator shows "insert before target", so:
        // - If dragging forward (source < target), final position is target - 1
        // - If dragging backward (source > target), final position is target (no adjustment needed)
        //
        // Example 1: Dragging forward [A, B, C] - drag A to position of C
        //   sourceIndex = 0, targetIndex = 2
        //   Visual: user sees drop indicator before C (wants A at index 2 after drop)
        //   Reality: removing A → [B, C], then insert at adjusted index 1 → [B, A, C]
        //   Formula: adjustedTargetIndex = 2 - 1 = 1 ✓
        //
        // Example 2: Dragging backward [A, B, C] - drag C to position of A
        //   sourceIndex = 2, targetIndex = 0
        //   Visual: user sees drop indicator before A (wants C at index 0 after drop)
        //   Reality: [A, B, C] → remove C → [A, B] → insert at index 0 → [C, A, B]
        //   Formula: adjustedTargetIndex = 0 (no adjustment) ✓
        const adjustedTargetIndex = sourceIndex < targetIndex ? targetIndex - 1 : targetIndex;
        reorderTab(tabId, adjustedTargetIndex, currentPaneId);
      } else {
        // Cross-pane move
        moveTabBetweenPanes(tabId, sourcePaneId, currentPaneId, targetIndex);
      }
    } catch (error) {
      console.error('[TabSystem] Drop operation failed:', error);
      // Could show user-facing toast notification here in the future
    } finally {
      // Always clear drag-over state
      dragOverIndex = null;
    }
  }

  /**
   * Check if a tab can be dragged
   * @param _tab - The tab to check (currently unused, parameter kept for future extensibility)
   * @returns false if this is the last tab in the last pane, true otherwise
   *
   * @remarks
   * Prevents dragging the last tab in the last pane to ensure the application
   * always has at least one visible tab. The tab parameter is reserved for future
   * per-tab drag restrictions (e.g., pinned tabs, locked tabs).
   */
  function canDragTab(_tab: Tab): boolean {
    // Cannot drag last tab in last pane
    return !(displayTabs.length === 1 && $tabState.panes.length === 1);
  }

  // Get active tab for slot prop
  const activeTab = $derived(displayTabs.find((tab) => tab.id === currentActiveTabId));
</script>

<!-- Tab bar - always shown (even with 1 tab for pane system) -->
<div class="tab-bar" role="tablist" tabindex="-1" aria-label="Content tabs">
  {#each displayTabs as tab, i (tab.id)}
    {@const isActive = tab.id === currentActiveTabId}
    {@const isDraggable = canDragTab(tab)}
    {@const showDropIndicator = dragOverIndex === i}
    {@const isDragging = draggingTabId === tab.id}

    <div
      class={cn(
        'tab-item',
        isActive && 'tab-item--active',
        showDropIndicator && 'tab-item--drop-target',
        isDragging && 'tab-item--dragging'
      )}
      role="tab"
      title={tab.title}
      tabindex={isActive ? 0 : -1}
      aria-selected={isActive}
      aria-controls={`tab-panel-${tab.id}`}
      aria-grabbed={isDragging}
      aria-dropeffect={showDropIndicator ? 'move' : 'none'}
      onkeydown={(event: KeyboardEvent) => handleTabKeydown(event, tab.id)}
      use:droppable={{
        container: `tab-${i}`,
        callbacks: {
          onDragEnter: handleDragEnter,
          onDragOver: handleDragOver,
          onDragLeave: handleDragLeave,
          onDrop: handleDrop
        }
      }}
    >
      <span
        class="tab-title"
        role="button"
        tabindex="0"
        onclick={() => handleTabClick(tab.id)}
        onkeydown={(e: KeyboardEvent) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            handleTabClick(tab.id);
          }
        }}
        use:draggable={{
          container: `tab-${i}`,
          dragData: { tab, paneId: currentPaneId },
          disabled: !isDraggable,
          callbacks: {
            onDragStart: handleDragStart,
            onDragEnd: handleDragEnd
          }
        }}
      >
        {truncateTitle(tab.title)}
      </span>

      <!-- Close button - only for closeable tabs, hidden when it's the last tab -->
      {#if tab.closeable && !isCloseDisabled}
        <button
          class="tab-close-btn"
          aria-label="Close tab: {tab.title}"
          title="Close tab"
          onclick={(e) => handleCloseTab(e, tab.id)}
        >
          <span class="close-icon"></span>
        </button>
      {/if}
    </div>
  {/each}

  <!-- Drop zone after last tab for "append to end" -->
  <div
    class={cn(
      'tab-end-drop-zone',
      dragOverIndex === displayTabs.length && 'tab-end-drop-zone--active'
    )}
    use:droppable={{
      container: `tab-${displayTabs.length}`,
      callbacks: {
        onDragEnter: handleDragEnter,
        onDragOver: handleDragOver,
        onDragLeave: handleDragLeave,
        onDrop: handleDrop
      }
    }}
  ></div>
</div>

<!-- Tab content area -->
<div
  class="tab-content"
  role="tabpanel"
  id={`tab-panel-${currentActiveTabId}`}
  aria-labelledby={`tab-${currentActiveTabId}`}
>
  {#if children}
    {@render children({ activeTab })}
  {/if}
</div>

<style>
  .tab-bar {
    display: flex;
    align-items: center;
    background-color: hsl(var(--tab-bar-background));
    padding: 0;
    margin: 0;
    overflow-x: auto;
    scrollbar-width: none;
    -ms-overflow-style: none;
    position: relative;
    /* Ensure tabs start from the very edge */
    margin-left: 0;
    padding-left: 0;
  }

  /* Add bottom border to empty space after tabs */
  .tab-bar::after {
    content: '';
    flex: 1;
    height: 1px;
    align-self: flex-end;
    border-bottom: 1px solid hsl(var(--border));
  }

  .tab-bar::-webkit-scrollbar {
    display: none;
  }

  .tab-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 16px;
    height: 40px;
    min-width: 0;
    flex-shrink: 0;
    cursor: pointer;
    background-color: hsl(var(--inactive-tab-background));
    color: hsl(var(--muted-foreground));
    font-weight: 500;
    font-size: 14px;
    outline: none;
    position: relative;
    box-sizing: border-box;
    line-height: 1.2;
    top: 0;
    border-left: 1px solid hsl(var(--border));
    border-bottom: 1px solid hsl(var(--border));
    border-right: 0; /* Remove right border except for the last tab */
    border-top: none; /* No top borders on tabs */
  }

  /* Remove left border from first tab to avoid double border */
  .tab-item:first-child {
    border-left: none;
  }

  /* Add right border to the last tab */
  .tab-item:last-child {
    border-right: 1px solid hsl(var(--border));
  }

  .tab-item:hover {
    background-color: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  /* Removed :focus-visible borders - Tab key used for indent/outdent, not UI navigation */

  .tab-item--active {
    background-color: hsl(var(--active-tab-background));
    color: hsl(var(--foreground));
    position: relative;
    box-sizing: border-box;
    line-height: 1.2;
    /* Identical properties to inactive tabs */
    padding: 0 16px;
    height: 40px;
    display: flex;
    align-items: center;
    gap: 8px;
    /* Ensure exact same positioning as inactive tabs */
    top: 0;
    /* Remove margin-bottom to prevent text shifting */
    margin-bottom: 0;
    /* Remove bottom border for active tab to connect with content */
    border-bottom: none !important;
  }

  /* Active tab styling */

  .tab-item--active:hover {
    background-color: hsl(var(--active-tab-background));
    color: hsl(var(--foreground));
  }

  .tab-title {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    min-width: 0;
    flex: 1;
  }

  /* Close button - positioned in upper right corner of tab */
  .tab-close-btn {
    position: absolute;
    top: 2px; /* Close to corner for compact appearance */
    right: 2px; /* Close to corner for compact appearance */
    display: flex;
    align-items: center;
    justify-content: center;
    width: 12px; /* Compact size to minimize visual intrusion */
    height: 12px; /* Compact size to minimize visual intrusion */
    padding: 0;
    background: none;
    border: none;
    color: hsl(var(--muted-foreground));
    cursor: pointer;
    opacity: 0; /* Hidden by default, shown on tab hover */
    transition: opacity 0.15s ease-in-out;
    border-radius: 2px; /* Subtle rounding to soften corners */
    z-index: 100; /* Ensure it's above drag handles */
    pointer-events: auto; /* Always receive pointer events */
  }

  /* CSS-only close icon (X shape using pseudo-elements) */
  .close-icon {
    position: relative;
    display: block;
    width: 8px; /* Small icon (67% fill ratio in 12px button) for subtle appearance */
    height: 8px; /* Small icon (67% fill ratio in 12px button) for subtle appearance */
  }

  .close-icon::before,
  .close-icon::after {
    content: '';
    position: absolute;
    top: 50%;
    left: 0;
    width: 100%;
    height: 1px; /* Thin line for crisp, delicate appearance */
    background-color: currentColor;
  }

  .close-icon::before {
    transform: translateY(-50%) rotate(45deg);
  }

  .close-icon::after {
    transform: translateY(-50%) rotate(-45deg);
  }

  /* Show close button when hovering over the tab */
  .tab-item:hover .tab-close-btn {
    opacity: 0.6;
  }

  /* Increase opacity when hovering directly over close button */
  .tab-close-btn:hover {
    opacity: 1 !important;
    color: hsl(var(--foreground));
  }

  /* Removed :focus-visible border - Tab key used for indent/outdent, not UI navigation */

  /* Disabled close button (last tab in last pane) */
  .tab-close-btn--disabled {
    opacity: 0.3 !important;
    cursor: not-allowed !important;
    pointer-events: none;
  }

  .tab-content {
    flex: 1;
    min-height: 0;
    background-color: hsl(var(--content-background));
    /* Remove top border to allow seamless connection with active tab */
    display: flex;
    flex-direction: column;
    overflow: hidden; /* Let children handle scrolling */
  }

  /* Ensure content fills the tab content area */
  .tab-content > :global(*) {
    flex: 1;
    min-height: 0;
  }

  /* Responsive behavior */
  @media (max-width: 768px) {
    .tab-item {
      padding: 8px 12px;
      font-size: 13px;
    }

    .tab-title {
      max-width: 120px;
    }
  }

  /* Drag-and-drop styles - minimal styling, library handles states */
  .tab-item {
    cursor: grab;
    user-select: none;
    -webkit-user-select: none;
  }

  .tab-item:active {
    cursor: grabbing;
  }

  /* Make the original tab more transparent while dragging */
  .tab-item--dragging {
    opacity: 0.4;
    transition: opacity 0.15s ease;
  }

  /* Drop target indicator - thicker left border */
  .tab-item--drop-target {
    border-left: 3px solid hsl(var(--primary)) !important;
    background-color: hsl(var(--primary) / 0.1);
    transition: all 0.15s ease;
  }

  /* First tab drop target - show border on right instead */
  .tab-item--drop-target:first-child {
    border-left: 1px solid hsl(var(--border));
    border-right: 3px solid hsl(var(--primary)) !important;
  }

  /* End drop zone - invisible area after last tab */
  .tab-end-drop-zone {
    min-width: 40px;
    height: 40px;
    flex-shrink: 0;
    position: relative;
    transition: all 0.15s ease;
  }

  /* Show visual indicator when hovering over end zone */
  .tab-end-drop-zone--active {
    background-color: hsl(var(--primary) / 0.1);
    border-left: 3px solid hsl(var(--primary));
  }

  /* Add a subtle hint that there's a drop zone */
  .tab-end-drop-zone--active::before {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    width: 20px;
    height: 20px;
    border: 2px dashed hsl(var(--primary));
    border-radius: 4px;
    opacity: 0.5;
  }
</style>
