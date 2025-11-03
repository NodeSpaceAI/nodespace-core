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
    moveTabBetweenPanes
  } from '$lib/stores/navigation.js';
  import { cn } from '$lib/utils.js';
  import type { Tab, Pane } from '$lib/stores/navigation.js';
  import type { Snippet } from 'svelte';

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
  // Use simple $derived instead of $derived.by for better reactivity
  const displayTabs = $derived(
    tabs ||
      (() => {
        const currentPane = $tabState.panes.find((p) => p.id === currentPaneId);
        if (!currentPane) return $tabState.tabs;

        // Order tabs according to pane's tabIds array
        return currentPane.tabIds
          .map((tabId) => $tabState.tabs.find((t) => t.id === tabId))
          .filter((t): t is Tab => t !== undefined);
      })()
  );

  const currentActiveTabId = $derived(
    activeTabId || $tabState.activeTabIds[$tabState.activePaneId]
  );

  // Check if close button should be disabled (last tab in last pane)
  const isCloseDisabled = $derived(displayTabs.length === 1 && $tabState.panes.length === 1);

  // Drag-and-drop state
  let draggedTabId: string | null = $state(null);
  let dragOverIndex: number | null = $state(null);
  let dragOverPaneId: string | null = $state(null);

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

  // Drag-and-drop handlers

  /**
   * Handle drag start - initiate dragging operation
   */
  function handleDragStart(event: DragEvent, tab: Tab, index: number): void {
    // Prevent dragging last tab in last pane
    if (displayTabs.length === 1 && $tabState.panes.length === 1) {
      event.preventDefault();
      return;
    }

    draggedTabId = tab.id;
    event.dataTransfer!.effectAllowed = 'move';
    event.dataTransfer!.setData(
      'application/nodespace-tab',
      JSON.stringify({
        tabId: tab.id,
        sourcePaneId: currentPaneId,
        sourceIndex: index
      })
    );

    // Set drag image (the tab element itself)
    const dragImage = event.currentTarget as HTMLElement;
    event.dataTransfer!.setDragImage(dragImage, 0, 0);
  }

  /**
   * Handle drag over - show drop zone and insertion point
   */
  function handleDragOver(event: DragEvent, index: number): void {
    event.preventDefault(); // Allow drop
    event.dataTransfer!.dropEffect = 'move';

    dragOverIndex = index;
    dragOverPaneId = currentPaneId;
  }

  /**
   * Handle drag leave - clear drop zone indicators
   */
  function handleDragLeave(event: DragEvent): void {
    // Only clear if leaving the tab bar entirely
    const relatedTarget = event.relatedTarget as HTMLElement;
    const tabBar = (event.currentTarget as HTMLElement).closest('.tab-bar');

    if (!tabBar?.contains(relatedTarget)) {
      dragOverIndex = null;
      dragOverPaneId = null;
    }
  }

  /**
   * Handle drop - perform tab reorder or move
   */
  function handleDrop(event: DragEvent, targetIndex: number): void {
    event.preventDefault();

    const data = event.dataTransfer?.getData('application/nodespace-tab');
    if (!data) {
      resetDragState();
      return;
    }

    const { tabId, sourcePaneId } = JSON.parse(data);

    if (sourcePaneId === currentPaneId) {
      // Within-pane reorder
      reorderTab(tabId, targetIndex, currentPaneId);
    } else {
      // Cross-pane move
      moveTabBetweenPanes(tabId, sourcePaneId, currentPaneId, targetIndex);
    }

    resetDragState();
  }

  /**
   * Handle drag end - cleanup drag state
   */
  function handleDragEnd(): void {
    resetDragState();
  }

  /**
   * Reset all drag-related state
   */
  function resetDragState(): void {
    draggedTabId = null;
    dragOverIndex = null;
    dragOverPaneId = null;
  }

  /**
   * Check if a tab can be dragged
   */
  function canDragTab(_tab: Tab): boolean {
    // Cannot drag last tab in last pane
    return !(displayTabs.length === 1 && $tabState.panes.length === 1);
  }

  // Get active tab for slot prop
  const activeTab = $derived(displayTabs.find((tab) => tab.id === currentActiveTabId));
</script>

<!-- Tab bar - always shown (even with 1 tab for pane system) -->
<div
  class={cn(
    'tab-bar',
    dragOverPaneId === currentPaneId && draggedTabId && 'tab-bar--drop-zone-active'
  )}
  role="tablist"
  tabindex="-1"
  aria-label="Content tabs"
  ondragleave={handleDragLeave}
>
  {#each displayTabs as tab, i (tab.id)}
    {@const isActive = tab.id === currentActiveTabId}
    {@const isDragging = draggedTabId === tab.id}
    {@const isDraggable = canDragTab(tab)}
    {@const showDropIndicator = dragOverIndex === i && dragOverPaneId === currentPaneId}

    <!-- Drop indicator before tab -->
    {#if showDropIndicator}
      <div class="drop-indicator"></div>
    {/if}

    <div
      class={cn('tab-item', isActive && 'tab-item--active', isDragging && 'tab-item--dragging')}
      role="tab"
      tabindex={isActive ? 0 : -1}
      aria-selected={isActive}
      aria-controls={`tab-panel-${tab.id}`}
      aria-grabbed={isDragging}
      draggable={isDraggable}
      onclick={() => handleTabClick(tab.id)}
      onkeydown={(event) => handleTabKeydown(event, tab.id)}
      ondragstart={(e) => handleDragStart(e, tab, i)}
      ondragover={(e) => handleDragOver(e, i)}
      ondrop={(e) => handleDrop(e, i)}
      ondragend={handleDragEnd}
    >
      <span class="tab-title" title={tab.title}>
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

    <!-- Drop indicator after last tab -->
    {#if i === displayTabs.length - 1 && dragOverIndex === displayTabs.length && dragOverPaneId === currentPaneId}
      <div class="drop-indicator"></div>
    {/if}
  {/each}
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

  /* Drag-and-drop styles */

  /* Draggable tab cursor */
  .tab-item[draggable='true'] {
    cursor: grab;
    user-select: none;
    -webkit-user-select: none;
  }

  .tab-item[draggable='true']:active {
    cursor: grabbing;
  }

  .tab-item[draggable='false'] {
    cursor: default;
  }

  /* Dragging state - show ghost effect */
  .tab-item--dragging {
    opacity: 0.4;
    pointer-events: none; /* Prevent hover effects during drag */
  }

  /* Drop zone highlight on tab bar */
  .tab-bar--drop-zone-active {
    background: hsl(var(--primary) / 0.05);
    outline: 2px dashed hsl(var(--primary) / 0.3);
    outline-offset: -2px;
    transition: all 0.15s ease;
  }

  /* Drop indicator - vertical line showing insertion point */
  .drop-indicator {
    position: absolute;
    width: 3px;
    height: calc(100% - 8px);
    top: 4px;
    background: hsl(var(--primary));
    border-radius: 1.5px;
    pointer-events: none;
    z-index: 100;
    box-shadow: 0 0 4px hsl(var(--primary) / 0.5);
    animation: pulse 0.6s ease-in-out infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.6;
    }
  }
</style>
