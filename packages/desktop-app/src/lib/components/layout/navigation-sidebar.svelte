<script lang="ts">
  import { Collapsible } from 'bits-ui';
  import {
    layoutState,
    navigationItems,
    toggleSidebar,
    setCollectionsExpanded
  } from '$lib/stores/layout.js';
  import { tabState, setActiveTab, addTab } from '$lib/stores/navigation.js';
  import {
    collectionsState,
    collectionsData,
    collectionsTree,
    selectedCollection,
    selectedCollectionMembers
  } from '$lib/stores/collections.js';
  import { formatDateISO } from '$lib/utils/date-formatting.js';
  import { v4 as uuidv4 } from 'uuid';
  import CollectionSubPanel from './collection-sub-panel.svelte';

  import { onMount } from 'svelte';

  // Subscribe to stores using Svelte 5 runes
  let isCollapsed = $derived($layoutState.sidebarCollapsed);
  let navItems = $derived($navigationItems);
  // Collections expanded state from layout store (persisted)
  let collectionsExpanded = $derived($layoutState.collectionsExpanded);

  // Collections state from collections store (UI-only, not persisted)
  let subPanelOpen = $derived($collectionsState.subPanelOpen);
  let expandedCollectionIds = $derived($collectionsState.expandedCollectionIds);

  // Collections data from backend
  let collections = $derived($collectionsTree);

  // Derived stores for sub-panel
  let collectionForPanel = $derived($selectedCollection);
  let collectionMembers = $derived($selectedCollectionMembers);

  // Load collections from backend on mount
  onMount(() => {
    collectionsData.loadCollections();
  });

  // Element references for click-outside detection
  let navElement: HTMLElement | null = $state(null);
  let subPanelElement: HTMLElement | null = $state(null);

  // Close sub-panel when sidebar collapses
  $effect(() => {
    if (isCollapsed && subPanelOpen) {
      collectionsState.clearSelection();
    }
  });

  // Click-outside handler for sub-panel dismissal
  $effect(() => {
    if (!subPanelOpen) return;

    function handleClickOutside(event: MouseEvent) {
      const target = event.target as Node;
      // Close if click is outside both nav and sub-panel
      const clickedOutsideNav = navElement && !navElement.contains(target);
      const clickedOutsideSubPanel = subPanelElement && !subPanelElement.contains(target);

      if (clickedOutsideNav && clickedOutsideSubPanel) {
        collectionsState.clearSelection();
      }
    }

    // Use capture phase to catch clicks before they bubble
    document.addEventListener('click', handleClickOutside, true);

    return () => {
      document.removeEventListener('click', handleClickOutside, true);
    };
  });

  function isCollectionExpanded(collectionId: string): boolean {
    return expandedCollectionIds.has(collectionId);
  }

  function handleCollectionClick(collectionId: string) {
    collectionsState.selectCollection(collectionId);
  }

  function handleCloseSubPanel() {
    collectionsState.closeSubPanel();
  }

  function handleNodeClick(nodeId: string, nodeType: string) {
    // Close sub-panel first
    handleCloseSubPanel();

    // Check if node is already open in a tab
    const currentState = $tabState;
    const existingTab = currentState.tabs.find((tab) => tab.content?.nodeId === nodeId);

    if (existingTab) {
      setActiveTab(existingTab.id, existingTab.paneId);
    } else {
      // Create new tab
      const targetPaneId = getTargetPaneId();
      addTab(
        {
          id: uuidv4(),
          title: 'Loading...', // Viewer will update
          type: 'node',
          content: { nodeId, nodeType },
          closeable: true,
          paneId: targetPaneId
        },
        true
      );
    }
  }

  /**
   * Get today's date in YYYY-MM-DD format
   */
  function getTodayDateId(): string {
    return formatDateISO(new Date());
  }

  /**
   * Get the target pane ID for new tabs
   * Uses active pane, or falls back to first available pane
   */
  function getTargetPaneId(): string {
    const currentState = $tabState;
    // Use active pane if it exists, otherwise use the first pane
    const paneExists = currentState.panes.some((p) => p.id === currentState.activePaneId);
    if (paneExists) {
      return currentState.activePaneId;
    }
    // Fallback to first pane (there should always be at least one)
    return currentState.panes[0]?.id ?? 'pane-1';
  }

  /**
   * Find if today's date is already open in any tab
   * @returns The tab with today's date if found, null otherwise
   */
  function findTodayDateTab() {
    const todayId = getTodayDateId();
    const currentState = $tabState;

    return currentState.tabs.find(
      (tab) => tab.content?.nodeId === todayId && tab.content?.nodeType === 'date'
    );
  }

  /**
   * Handle Daily Journal navigation
   * 1. First look for existing tab with today's date
   * 2. If found, make it active
   * 3. If not found, create new tab in the active pane (or first available pane)
   */
  function handleDailyJournalClick() {
    const existingTab = findTodayDateTab();

    if (existingTab) {
      // Tab with today's date found - activate it
      setActiveTab(existingTab.id, existingTab.paneId);
    } else {
      // No tab with today's date - create new one in active/first pane
      // Title is a placeholder - DateNodeViewer sets the real title on mount
      const todayId = getTodayDateId();
      const targetPaneId = getTargetPaneId();
      const newTab = {
        id: uuidv4(),
        title: todayId, // Placeholder - viewer will update to "Today" on mount
        type: 'node' as const,
        content: {
          nodeId: todayId,
          nodeType: 'date'
        },
        closeable: true,
        paneId: targetPaneId
      };

      addTab(newTab, true); // Make it active
    }
  }

  // Handle navigation item clicks
  function handleNavItemClick(itemId: string) {
    // Close sub-panel when clicking non-collection nav items
    if (subPanelOpen) {
      collectionsState.clearSelection();
    }

    // Special handling for Daily Journal
    if (itemId === 'daily-journal') {
      handleDailyJournalClick();
    }

    // Update active state in navigation items
    navigationItems.update((items) =>
      items.map((item) => ({
        ...item,
        active: item.id === itemId
      }))
    );
  }
</script>

<nav
  bind:this={navElement}
  class="sidebar navigation-sidebar"
  class:sidebar-collapsed={isCollapsed}
  class:sidebar-expanded={!isCollapsed}
>
  <!-- Hamburger menu button -->
  <button
    class="hamburger-button"
    onclick={toggleSidebar}
    aria-label={isCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}
    aria-expanded={!isCollapsed}
  >
    <svg
      class="hamburger-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
    >
      <line x1="3" y1="6" x2="21" y2="6"></line>
      <line x1="3" y1="12" x2="21" y2="12"></line>
      <line x1="3" y1="18" x2="21" y2="18"></line>
    </svg>
  </button>

  <!-- Navigation items -->
  <div class="nav-items">
    <!-- Daily Journal (first item) -->
    {#each navItems.slice(0, 1) as item}
      <button
        class="nav-item"
        onclick={() => handleNavItemClick(item.id)}
        aria-label={item.label}
        disabled={item.type === 'placeholder'}
        title={isCollapsed ? item.label : undefined}
      >
        <svg
          class="nav-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <path d={item.icon}></path>
        </svg>
        {#if !isCollapsed}
          <span class="nav-label">{item.label}</span>
        {/if}
      </button>
    {/each}

    <!-- Collections section (after Daily Journal) - accordion toggle -->
    {#if !isCollapsed}
      <Collapsible.Root open={collectionsExpanded} onOpenChange={(open) => setCollectionsExpanded(open)}>
        <Collapsible.Trigger class="nav-item">
          <svg
            class="nav-icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
          >
            <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
          </svg>
          <span class="nav-label">Collections</span>
        </Collapsible.Trigger>

        <Collapsible.Content>
          <div class="collection-list">
            {#each collections as collection (collection.id)}
              {@const hasChildren = collection.children && collection.children.length > 0}
              {@const isExpanded = isCollectionExpanded(collection.id)}
              <div class="collection-item">
                {#if hasChildren}
                  <button
                    class="expand-btn"
                    onclick={() => collectionsState.toggleCollectionExpanded(collection.id)}
                    aria-label={isExpanded ? 'Collapse' : 'Expand'}
                  >
                    <svg
                      class="expand-chevron"
                      class:rotate-90={isExpanded}
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="2"
                    >
                      <path d="M9 18l6-6-6-6" />
                    </svg>
                  </button>
                {/if}
                <button
                  class="collection-name-btn"
                  onclick={() => handleCollectionClick(collection.id)}
                >
                  {collection.name}
                </button>
              </div>

              <!-- Level 2 -->
              {#if hasChildren && isExpanded && collection.children}
                {#each collection.children as child (child.id)}
                  {@const childHasChildren = child.children && child.children.length > 0}
                  {@const childIsExpanded = isCollectionExpanded(child.id)}
                  <div class="collection-item level-2">
                    {#if childHasChildren}
                      <button
                        class="expand-btn"
                        onclick={() => collectionsState.toggleCollectionExpanded(child.id)}
                        aria-label={childIsExpanded ? 'Collapse' : 'Expand'}
                      >
                        <svg
                          class="expand-chevron"
                          class:rotate-90={childIsExpanded}
                          viewBox="0 0 24 24"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="2"
                        >
                          <path d="M9 18l6-6-6-6" />
                        </svg>
                      </button>
                    {/if}
                    <button
                      class="collection-name-btn"
                      onclick={() => handleCollectionClick(child.id)}
                    >
                      {child.name}
                    </button>
                  </div>

                  <!-- Level 3 -->
                  {#if childHasChildren && childIsExpanded && child.children}
                    {#each child.children as grandchild (grandchild.id)}
                      <div class="collection-item level-3">
                        <button
                          class="collection-name-btn"
                          onclick={() => handleCollectionClick(grandchild.id)}
                        >
                          {grandchild.name}
                        </button>
                      </div>
                    {/each}
                  {/if}
                {/each}
              {/if}
            {/each}
          </div>
        </Collapsible.Content>
      </Collapsible.Root>
    {:else}
      <!-- Collapsed state: just show icon -->
      <button
        class="nav-item"
        title="Collections"
        onclick={() => {
          toggleSidebar();
          setCollectionsExpanded(true);
        }}
      >
        <svg
          class="nav-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
        </svg>
      </button>
    {/if}

    <!-- Remaining nav items (Search, Favorites) -->
    {#each navItems.slice(1) as item}
      <button
        class="nav-item"
        onclick={() => handleNavItemClick(item.id)}
        aria-label={item.label}
        disabled={item.type === 'placeholder'}
        title={isCollapsed ? item.label : undefined}
      >
        <svg
          class="nav-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <path d={item.icon}></path>
        </svg>
        {#if !isCollapsed}
          <span class="nav-label">{item.label}</span>
        {/if}
      </button>
    {/each}
  </div>

  <!-- Collection sub-panel -->
  <div bind:this={subPanelElement}>
    <CollectionSubPanel
      open={subPanelOpen}
      collectionName={collectionForPanel?.content ?? collectionForPanel?.name ?? ''}
      members={collectionMembers}
      onClose={handleCloseSubPanel}
      onNodeClick={handleNodeClick}
    />
  </div>
</nav>

<style>
  .sidebar {
    background: hsl(var(--sidebar-background));
    border-right: 1px solid hsl(var(--border));
    display: flex;
    flex-direction: column;
    height: 100vh;
    position: relative;
  }

  /* Sidebar transition animation configured in app.css */

  /* Collapsed sidebar - adjusted width to center hamburger menu */
  .sidebar-collapsed {
    width: 52px; /* Width to center hamburger: 8px left + 16px padding + 20px icon + 8px right = 52px */
    align-items: stretch; /* Don't center content - let nav items control their own alignment */
    padding: 1rem 0; /* Remove left padding - center everything */
  }

  /* Expanded sidebar - exact specifications from patterns.html */
  .sidebar-expanded {
    width: 240px; /* Good balance between functionality and size */
    padding: 1rem;
  }

  /* Hamburger button - base styles moved to positioning section */

  .hamburger-button:hover {
    color: hsl(var(--foreground));
  }

  /* Removed :focus-visible border - Tab key used for indent/outdent, not UI navigation */

  .hamburger-icon {
    width: 20px;
    height: 20px;
    flex-shrink: 0;
  }

  /* Hamburger button - keep at fixed position, adjust sidebar width instead */
  .hamburger-button {
    background: none;
    border: none;
    cursor: pointer;
    padding: 0.5rem;
    transition: color 0.2s;
    color: hsl(var(--foreground));
    position: absolute;
    left: 0.5rem; /* Fixed 8px from left edge */
    top: 1rem; /* Fixed top position */
    z-index: 10; /* Ensure it stays above nav items */
  }

  /* Navigation items container */
  .nav-items {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 0; /* Remove gaps between nav items */
    margin-top: 3rem; /* Space for absolutely positioned hamburger button */
  }

  /* Navigation items - Full-width navigation items with no gaps */
  .nav-item {
    background: none;
    border: none;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: 0.75rem;
    height: 40px; /* Fixed height for consistent alignment */
    box-sizing: border-box;
    transition:
      background-color 0.2s,
      color 0.2s;
    color: hsl(var(--muted-foreground));
    position: relative;
    text-align: left;
    font-weight: 500;
  }

  .nav-item:hover:not(:disabled) {
    background: hsl(var(--border)); /* Use border color for more visible hover effect */
    color: hsl(var(--foreground));
  }

  /* Removed :focus-visible border - Tab key used for indent/outdent, not UI navigation */

  .nav-item:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  /* Collapsed sidebar specific nav items - align icons at 16px from left edge */
  .sidebar-collapsed .nav-item {
    display: flex;
    align-items: center;
    justify-content: flex-start; /* Align left instead of center */
    margin: 0;
    width: 100%; /* Full width for background highlights */
    padding: 0.5rem 0; /* Vertical padding only - NO horizontal padding */
    padding-left: 1rem; /* This should put icon at 16px from edge */
    position: relative;
    box-sizing: border-box;
  }

  /* Collapsed sidebar - no special active styling */

  /* Expanded sidebar nav items */
  .sidebar-expanded .nav-item {
    /* Full-width navigation items with no gaps */
    margin: 0 -1rem;
    padding: 0.5rem 1rem;
    border-radius: 0;
  }

  /* No special active styling - nav items are just for navigation */

  /* Navigation icon */
  .nav-icon {
    width: 20px;
    height: 20px;
    flex-shrink: 0;
  }

  /* Navigation label */
  .nav-label {
    font-size: 0.875rem;
    white-space: nowrap;
    transition: opacity 0.2s ease-out;
  }

  /* Collections trigger styling - full-width hover to match nav items */
  .sidebar-expanded :global([data-collapsible-trigger]) {
    margin: 0 -1rem;
    padding: 0.5rem 1rem;
    width: calc(100% + 2rem); /* Full width including negative margins */
    display: flex;
    align-items: center;
    gap: 0.75rem;
    background: none;
    border: none;
    cursor: pointer;
    color: hsl(var(--muted-foreground));
    font-weight: 500;
    font-size: 0.875rem;
    text-align: left;
    height: 40px;
    box-sizing: border-box;
    transition:
      background-color 0.2s,
      color 0.2s;
  }

  .sidebar-expanded :global([data-collapsible-trigger]:hover) {
    background: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  /* Chevron icon removed - Collections is now a simple accordion toggle */

  /* Collection list (expanded content) */
  .collection-list {
    display: flex;
    flex-direction: column;
    padding: 0; /* No gap between Collections header and sub-items */
    margin: 0 -1rem; /* Break out of sidebar padding */
    overflow-x: auto; /* Allow horizontal scroll for long names */
    scrollbar-width: none; /* Firefox */
    -ms-overflow-style: none; /* IE/Edge */
    background: hsl(var(--active-nav-background)); /* Subtle background to group collection items */
  }

  .collection-list::-webkit-scrollbar {
    display: none; /* Chrome/Safari/Opera */
  }

  /* Collection item - container for expand button and name */
  .collection-item {
    display: flex;
    align-items: center;
    gap: 0;
    min-width: 100%; /* Allow growing beyond container width */
    width: max-content; /* Size to content for horizontal scrolling */
    padding: 0 1rem 0 3.5rem; /* Indent clearly inside Collections group */
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    transition:
      background-color 0.2s,
      color 0.2s;
  }

  .collection-item:hover {
    background: hsl(var(--border));
    color: hsl(var(--foreground));
  }

  /* Nested level indentation */
  .collection-item.level-2 {
    padding-left: 4.25rem;
  }

  .collection-item.level-3 {
    padding-left: 5rem;
  }

  /* Expand/collapse button */
  .expand-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 28px;
    background: none;
    border: none;
    cursor: pointer;
    color: inherit;
    padding: 0;
    flex-shrink: 0;
  }

  .expand-btn:hover {
    color: hsl(var(--foreground));
  }

  /* Collection name button - takes remaining space */
  .collection-name-btn {
    flex: 1;
    background: none;
    border: none;
    cursor: pointer;
    color: inherit;
    text-align: left;
    padding: 0.4rem 0;
    font-size: inherit;
    white-space: nowrap; /* Keep on single line, scroll horizontally if needed */
  }

  /* Expand chevron inside collection item */
  .expand-chevron {
    width: 12px;
    height: 12px;
    flex-shrink: 0;
    transition: transform 0.15s ease-out;
  }

  .expand-chevron.rotate-90 {
    transform: rotate(90deg);
  }
</style>
