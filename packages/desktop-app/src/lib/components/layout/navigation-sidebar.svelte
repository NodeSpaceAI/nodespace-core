<script lang="ts">
  import { layoutState, navigationItems, toggleSidebar } from '$lib/stores/layout.js';
  import { tabState, setActiveTab, addTab, DEFAULT_PANE_ID } from '$lib/stores/navigation.js';
  import { formatDateISO } from '$lib/utils/date-formatting.js';
  import { v4 as uuidv4 } from 'uuid';

  // Subscribe to stores
  $: isCollapsed = $layoutState.sidebarCollapsed;
  $: navItems = $navigationItems;

  /**
   * Get today's date in YYYY-MM-DD format
   */
  function getTodayDateId(): string {
    return formatDateISO(new Date());
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
   * 3. If not found, create new tab in left pane (pane-1)
   */
  function handleDailyJournalClick() {
    const existingTab = findTodayDateTab();

    if (existingTab) {
      // Tab with today's date found - activate it
      setActiveTab(existingTab.id, existingTab.paneId);
    } else {
      // No tab with today's date - create new one in left pane
      const todayId = getTodayDateId();
      const newTab = {
        id: uuidv4(),
        title: 'Daily Journal',
        type: 'node' as const,
        content: {
          nodeId: todayId,
          nodeType: 'date'
        },
        closeable: true,
        paneId: DEFAULT_PANE_ID // Always create in left pane
      };

      addTab(newTab, true); // Make it active
    }
  }

  // Handle navigation item clicks
  function handleNavItemClick(itemId: string) {
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
  class="sidebar navigation-sidebar"
  class:sidebar-collapsed={isCollapsed}
  class:sidebar-expanded={!isCollapsed}
>
  <!-- Hamburger menu button -->
  <button
    class="hamburger-button"
    on:click={toggleSidebar}
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
    {#each navItems as item}
      <button
        class="nav-item"
        class:active={item.active}
        on:click={() => handleNavItemClick(item.id)}
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

  /* Collapsed sidebar active item - full-width background with left accent */
  .sidebar-collapsed .nav-item.active {
    background: hsl(var(--active-nav-background));
    color: hsl(var(--foreground)); /* Same brightness as expanded state */
    width: 100%; /* Full width of collapsed sidebar (52px) */
    margin: 0; /* Remove any margin that might narrow the background */
    padding: 0.5rem 0; /* Keep vertical padding */
    padding-left: 1rem; /* Maintain 16px left padding to align with hamburger */
    position: relative;
  }

  .sidebar-collapsed .nav-item.active::before {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0; /* Accent at the very left edge of the sidebar */
    width: 4px;
    background: hsl(var(--primary));
    /* Accent overlays without pushing content */
  }

  /* Expanded sidebar nav items */
  .sidebar-expanded .nav-item {
    /* Full-width navigation items with no gaps */
    margin: 0 -1rem;
    padding: 0.5rem 1rem;
    border-radius: 0;
  }

  .sidebar-expanded .nav-item.active {
    background: hsl(var(--active-nav-background));
    color: hsl(var(--foreground));
    border-left: 4px solid hsl(var(--primary));
    padding-left: calc(1rem - 4px);
  }

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
</style>
